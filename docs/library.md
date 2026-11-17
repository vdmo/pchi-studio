# The show library: save and plan shows on the server

Until now a show lived in a file on one person's machine (Studio's **Save** downloads a `.pchi`) or in that browser's storage. The **library** keeps shows on the server, so a crew can save a show, plan it (a date, a venue, a status, notes), and open it from any machine that can reach the runtime. It is separate from the show that is *running*: the library holds shows being built and planned; the Live tab runs one.

## Turn it on

```bash
pchi-studio-api my-show.pchi --roster people.json --library /srv/pchi/shows
```

`--library DIR` (or `PCHI_LIBRARY`) keeps the library in that folder; it is created if missing. Pair it with a **roster** ([roles.md](roles.md)): the library uses it to know whose show is whose. Without a roster the runtime says so at startup, and everyone is the same person, so every show is everyone's.

## Using it in Studio

The **Library** button in the top bar opens the dialog.

- **Open and plan**: every show you may see, planned shows still to come first (soonest at the top). Open one into the editor, make a private **Copy** of one you do not own, or **Remove** one of yours.
- **Save this show**: name, planned date and time, venue, status (Draft, Planned, Ready, Done), who can see it, and notes. *Save to the server* the first time; after that *Save new version* adds the next version, *Update details only* changes the date or notes without saving the show, and *Save as a new show* branches it.
- A **server v3** mark in the top bar says the show in the editor came from the server and which version it was built on.
- **Save** and **Open** (files) are still there and work as before.

If the runtime has a roster, the dialog asks you to sign in with your own key first (the same key as the Live tab).

## Example shows

[tools/examples](../tools/examples/README.md) has three stage shows at different scales (a stadium pop tour, a condensed arena show, and a stadium doors and pre-show day) with invented content. Add them to a library with `node tools/examples/import-examples.mjs http://127.0.0.1:3101` (once each: running it again leaves them alone). They are marked as examples in their names and notes, so they are easy to tell from real shows and to remove.

## Who may do what

| | Sees | Saves / changes | Removes |
|---|---|---|---|
| **operator** | every show | any show | any show |
| **editor** | own shows and shows marked *team* | own shows | own shows |
| **guest**, button | shows marked *team* | nothing | nothing |
| **agent** | nothing | nothing | nothing |
| nobody signed in (a locked runtime) | nothing | nothing | nothing |

- A show is **private** (only its owner and operators) until its owner marks it **team** (everyone who signs in). An editor who cannot change someone's show can make a **copy** and build on that.
- A show you may not see is reported as "no such show": its existence is not shown to you.
- Agents have no library access. An agent proposes edits to the *running* show for a person to approve ([mcp.md](mcp.md)); it does not read people's drafts.
- Saves, copies and removals are written to the receipts like other decisions ([governance.md](governance.md)).

## Two people, one show

Every save says which version it was built on. If someone saved a newer one in between, the server **refuses** and says who ("Alex saved version 4 since you opened it"): nothing is overwritten. Open the latest, or save yours as a copy. This is a safety net for two people, not a merge: PCHI does not combine two edits.

## What is kept

```
<library>/<show-id>/meta.json     name, owner, visibility, status, planned date, venue, notes, version
<library>/<show-id>/v0001.pchi    one file per saved version
<library>/_archive/               shows that were removed (put one back by moving its folder out)
```

- A save adds a version; nothing is overwritten. The last **50** versions of each show are kept.
- Files are written to a temporary name and renamed into place, and `meta.json` last, so a crash leaves the last complete save.
- A show file may be up to 8 MB.
- **Device passwords are not stored.** If a show's rig has a `password` (an OBS node's), the server takes it out and Studio tells you which device. Set it on the machine that talks to the device (`PCHI_OBS_PASSWORD`). Other things in a show file are kept as they are: a show *marked team* is readable by everyone who signs in, so do not put secrets in notes or device parameters.

## Over HTTP

For scripts and other tools. All need the usual key or session header, and the runtime's `--token` if it has one.

| | |
|---|---|
| `GET /library` | the shows you may see |
| `GET /library/<id>` (`?v=N`) | the details, the versions that exist, and the show file |
| `POST /library` | `{"name", "show": {...}, "planned_for", "venue", "status", "notes", "visibility"}` keeps a new show |
| `PUT /library/<id>` | `{"base_version": N, "show": {...}}` adds a version; any of the detail fields alone change just those. `409` if N is no longer the latest |
| `POST /library/<id>/copy` | a private copy of your own |
| `DELETE /library/<id>` | removes it to `_archive` |

## Making it work for other people

The library lives where the runtime runs, so other users need to reach that runtime:

1. Run `pchi-studio-api` on a machine they can reach, with `--roster`, `--library`, and `--token` if the network is shared.
2. Put HTTPS in front of it ([tls.md](tls.md)): sign-in keys and session tokens otherwise cross the network in clear.
3. Give each person their own key (`--new-roster-entry Name:editor`) and a role.
4. They open Studio, press **Library**, sign in, and save.

Back the library folder up like any other folder: it is the only copy.

## What this does not do

- **No merge** of two people's edits (see above), and no live co-editing: it is save, and open.
- **No automatic versioning of the running show.** Sending a show to the runtime (Live tab) and saving to the library are separate steps.
- **Sessions and keys are the roster's**: one shared key means one person. There is no single sign-on, no teams or groups beyond "private" and "team", and no per-show list of people.
- **Studio's Library and the runtime's address:** Studio talks to the runtime it is pointed at (`/api`); a library on a second runtime is a separate library.
- **Tested** with unit tests of the storage, and over HTTP with an operator, two editors and a guest, a stale save, path tricks and an agent header; in Studio by one browser session. Not tested under load, with many shows, or on a network share.
