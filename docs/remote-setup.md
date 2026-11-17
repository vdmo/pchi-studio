# Remote setup: one venue computer, reached over Tailscale

The picture: one computer at the venue runs the runtime and talks to the rig on the venue network. People who are somewhere else reach **that computer** over a private network (Tailscale), open Studio in a browser, sign in with their own key, and plan, edit and watch. The cues still go from the venue computer to the tools over the venue LAN: nothing about the rig is exposed to the internet.

```
 remote laptop ──(Tailscale, encrypted)──► venue computer ──(venue LAN: OSC/UDP, Art-Net, obs-websocket)──► tools
   browser: Studio                          Caddy :8080  ► runtime 127.0.0.1:3100
```

- The runtime listens on **loopback only**. The one way in from other machines is a small web server (Caddy) bound to the **Tailscale address only**. It serves Studio and passes `/api` to the runtime.
- Tailscale encrypts the traffic between machines and only lets in devices on your own tailnet, so there is no public port and no certificate to manage.
- Who can do what is the **roster** ([roles.md](roles.md)): each person has their own key and a role.

> **What has and has not been tried.** The scripts and the roster, library, memory, receipts and resume were run for real (the venue computer part, on loopback). **Tailscale and Caddy were not installed where this was written**, so the Tailscale steps, the Caddy config's behaviour (serving Studio, passing `/api`, the live feed's WebSocket) and a connection from a second machine are **written from their documentation and not run**. Do the check at the end before you rely on it.

## 1. Before you start

On the venue computer you need: the runtime (`pchi-studio-api`), Python 3, a show file, and Studio built once:

```bash
cd pchi-studio-react && npm ci && npm run build        # makes pchi-studio-react/dist
```

Windows with WSL: Tailscale for Windows does **not** reach programs inside WSL2 (WSL has its own address behind NAT; Windows 10 has no "mirrored" mode). Install Tailscale **inside WSL** as below. Reaching OBS or TiXL *on Windows* from the runtime in WSL still uses WSL's gateway address, as in [tools/tixl/README.md](../tools/tixl/README.md) and [obs-plugin.md](obs-plugin.md).

## 2. Tailscale on the venue computer

```bash
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscaled &                           # if the system does not start it for you (WSL often does not)
sudo tailscale up --hostname=venue-1        # prints a link: open it and sign in
tailscale ip -4                             # its Tailscale address, 100.x.y.z
```

In Tailscale's admin console (login.tailscale.com), for the `venue-1` machine choose **Disable key expiry**, or it drops off the network when its key runs out, in the middle of a tour.

**Each remote person:** install Tailscale on their laptop and sign in to the **same tailnet** (invite them in the admin console, or use **Share** for one machine to someone outside your tailnet). Their laptop can then reach `venue-1`.

### Limit who can reach it (recommended)

By default every device on a tailnet can reach every other. Say that only the crew may reach the venue computer's web door. In the admin console, **Access controls**:

```json
{
  "groups":    { "group:crew": ["alex@example.com", "eddie@example.com"] },
  "tagOwners": { "tag:venue": ["autogroup:admin"] },
  "acls": [
    { "action": "accept", "src": ["group:crew"], "dst": ["tag:venue:8080"] }
  ]
}
```

Then give the venue machine that tag: `sudo tailscale up --hostname=venue-1 --advertise-tags=tag:venue`. (If your tailnet already has rules, add these lines to them rather than replacing the file: a policy that is replaced can lock people out.)

## 3. Caddy on the venue computer

Install it from <https://caddyserver.com/docs/install>. If your package manager also starts a Caddy service of its own, stop it: it would take port 80 and read a different config.

```bash
sudo systemctl disable --now caddy        # only if the installer started one
```

`start-venue.sh` writes the Caddy config itself and runs Caddy; you do not write one by hand. For reference it is:

```
http://100.x.y.z:8080 {
	bind 100.x.y.z
	handle_path /api/* { reverse_proxy 127.0.0.1:3100 }
	handle { root * /path/to/pchi-studio-react/dist  try_files {path} /index.html  file_server }
}
```

## 4. The roster: who may do what

```bash
cp tools/venue/venue.conf.example tools/venue/venue.conf      # then edit: SHOW, DATA, BIN, PROFILES
bash tools/venue/add-person.sh Alex   operator
bash tools/venue/add-person.sh Eddie  editor
bash tools/venue/add-person.sh Sam    editor lighting         # an editor limited to lighting
bash tools/venue/add-person.sh Gus    guest                   # can look, cannot change
```

Each command prints that person's key **once**. Give it to them privately (not in a shared chat). The roster file (`$DATA/roster.json`, owner-only) holds only a salted hash of each key. Roles are in [roles.md](roles.md); for remote work:

| Remote person | Give them |
|---|---|
| plans and edits shows | `editor` (or `editor` limited to their department) |
| watches, a client | `guest` |
| presses GO from a distance | `operator` only if you mean it: GO has a delay of tens to hundreds of ms over the internet |
| arms lasers | nobody remote. Leave that to the laser safety officer (`lso`) on site |

## 5. Start the venue computer

```bash
bash tools/venue/start-venue.sh
```

It refuses to start without a roster that has an operator, then starts the runtime on `127.0.0.1:3100` with the roster, the [show library](library.md), [memory](memory.md), signed receipts ([governance.md](governance.md)), a run log, and kept sessions. If the computer restarted mid-run it resumes the run that was going (`--resume`). Then, if it finds a Tailscale address and a built Studio, it starts Caddy and prints where Studio is:

```
remote: Studio is at http://100.x.y.z:8080/ for people on your Tailscale network
```

- **Dry run by default.** Cues are *reported, not sent* until you set `SEND=1` in `venue.conf`. Do that when the rig is ready and you have run the [safety drill](safety-drill.md).
- **Where things are kept**: everything is under `DATA` (default `~/pchi-venue`): `roster.json`, `library/`, `memory.db`, `show.db`, `governance/`, `secret/governance.key`, `logs/`. **Back this folder up**, and keep a copy of the receipt **verifying key** that is printed at startup somewhere the venue computer cannot change.
- **Adding someone later**: run `add-person.sh`, then restart (`Ctrl+C`, run `start-venue.sh` again): the roster is read at startup. The run resumes.
- **Keep it running.** `start-venue.sh` stays in the foreground: run it in a terminal you leave open, in `tmux`/`screen`, or as a service (a systemd unit running `bash /path/to/tools/venue/start-venue.sh` as your user, restarting on failure). A service was not tried here.

## 6. A remote person's first connection

1. Install Tailscale, sign in to the tailnet.
2. Open `http://venue-1:8080/` (the machine name; or the `100.x.y.z` address).
3. Press **Library** (or the **Live** tab) and sign in with your own key. Save and plan shows in the Library; the Live tab shows the running show.

## Check it before you rely on it

From a **second machine** on the tailnet, not the venue computer:

- [ ] `http://venue-1:8080/` shows Studio, and a reload on a deep link still shows it.
- [ ] Sign in with a key; the Live tab says "runtime: online" and the scene changes when someone on site moves it (that proves the WebSocket).
- [ ] A **guest** key cannot save to the Library and cannot press GO.
- [ ] A machine **not** on the tailnet (a phone off Wi-Fi without Tailscale) cannot reach it.
- [ ] `http://<the venue computer's LAN address>:8080/` does **not** answer from another machine on the venue LAN (the web door is only on the Tailscale address), and nothing answers on `:3100`.
- [ ] Stop and restart `start-venue.sh`: people stay signed in (the script keeps sessions in a file), and the show resumes where it was.

## What this does not cover

- **Not the failover pair.** One venue computer is one point of failure. A standby ([failover.md](failover.md)) is a separate setup; running it across Tailscale would put the failover key on that link: it is encrypted, but untried.
- **Anyone on your tailnet who gets past the access rules** can open the web door and see the show; a key is still needed to change anything. A shared or leaked key is still that person ([roles.md](roles.md)). Remove someone by deleting their entry and restarting, and remove their device in Tailscale.
- **GET requests that show state need no key** (the live view reads state without signing in). Keep the tailnet small, or use the access rules above, or add `--token`.
- **No public address.** If a person cannot install Tailscale, the alternative is HTTPS behind a proxy ([tls.md](tls.md)): more exposure, and it needs a name and certificate.
- **Remote GO is delayed**, and a remote person cannot see or hear the room. Safety-critical actions (lasers, strobes) stay with someone on site.
