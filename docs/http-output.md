# HTTP output: web requests from a closed list

A rig device can answer a cue by making **one web request** to a device on the network: Bitfocus Companion's button API, a camera or media server with a web interface, a webhook on the show network. The requests come from a **profile file** (`"transport": "http"`), not from the cue.

## What a cue can and cannot do

A cue supplies only a **value**. The profile says the method, the path and the body; the rig says the host and port.

- The value must match the action's pattern, and is then percent-encoded into the path or escaped into a JSON body. A value such as `1/2/3?x=1`, `../../admin` or one with a line break is refused before anything is sent.
- A cue cannot choose the host, the path, the method, the headers or the body, so it cannot make PCHI call an address it was not set up for.
- Methods are `GET`, `POST` and `PUT`. No `DELETE`, no `PATCH`. A profile for the laser or motion departments is refused: those need an acknowledgement and a stop, which a web request does not give.

## What it does not do

- **Plain `http://` only.** No TLS, so nothing goes to a public web service and an HTTPS-only device cannot be driven.
- No redirects, cookies, authentication headers, or reading the answer's body. Only the status line is read.
- One request per cue, 2 s to connect, 4 s to answer. A failure is reported and never stops the show.
- **`accepted` means the device answered 2xx to the request.** It does not mean the light, camera or button did what you wanted.

## Built in: Companion

`companion-http` (Rig tab: "Companion (HTTP)", or `add companion http`), port 8000.

| Cue kind | Value | Sends |
|---|---|---|
| `visual.button` | `page/row/column`, e.g. `1/2/3` | `POST /api/location/1/2/3/press` |
| `visual.button_down` | same | `POST /api/location/1/2/3/down` |
| `visual.button_up` | same | `POST /api/location/1/2/3/up` |

**Basis: memory.** The route is as remembered from Companion's documentation, with no copy read and no running Companion tried. Companion 2 used a different route (`GET /press/bank/<page>/<button>`) that this profile does not send. Check the path against your version's documentation. The existing `companion` type, which speaks OSC, is separate.

## Writing your own (a camera, a webhook)

Put a JSON file in the folder given to `--profiles`:

```json
{
  "id": "stage-cam",
  "name": "Stage camera",
  "transport": "http",
  "dept": "visual",
  "port": 80,
  "kinds": ["visual.preset", "visual.home"],
  "hint": "a preset number, 1 to 99",
  "notes": "From the camera's own manual, page 12.",
  "actions": {
    "visual.preset": {
      "value": "([1-9][0-9]?)",
      "method": "GET",
      "path": "/cgi-bin/preset?recall={1}",
      "basis": "vendor",
      "source": "Stage camera HTTP manual, section 4.2"
    },
    "visual.home": {
      "method": "POST",
      "path": "/api/home",
      "body": "{{\"speed\":\"fast\"}}",
      "content_type": "application/json",
      "basis": "vendor",
      "source": "Stage camera HTTP manual, section 4.5"
    }
  }
}
```

- Placeholders in a path or body: `{0}` is the whole value, `{1}` to `{9}` are the bracketed parts of the value's pattern. A literal brace is written twice (`{{` and `}}`), as in the JSON body above.
- A path has letters, digits and `/ . _ ~ - ? & = :`. A body is up to 500 plain characters, with `content_type` `application/json` or `text/plain`.
- An action with no `value` takes no value.
- `basis` is `vendor`, `community` or `memory`, with a `source`. It cannot be `proven`: that word is for a tool that has been seen doing the right thing ([gear-check.md](gear-check.md)).
- Then add a device of type **OSC** (or any) with the parameter `profile` set to `stage-cam` and its `host` set, as for a [link profile](link-profiles.md). The cue kinds are not yet offered in the Cue sheet's menus: write them in the show file or send them from the command line.

The Doors check refuses a cue whose value the profile does not accept, naming the reason, and a dry run shows the request line that would be sent.

## What is tested

The profile rules (everything above that is refused), the exact request bytes, encoding and escaping of values, the Doors check and dry-run text, and a **stand-in web server** that receives the request and answers 200, 404, not-HTTP and nothing at all. **Not tested: a real Companion, camera or media server.**
