# Virtual events: OBS and the audience

Two things take PCHI beyond the room: a cue that **cuts the picture** in OBS Studio, and an **audience** that can vote on what happens next.
Both follow the same rules as everything else: a device is checked before the show, a refusal is written down, and the audience can do
nothing but vote.

## OBS Studio

Add a device of type **OBS Studio** in the Rig tab (department: visual). A `scene.activate` cue to it names an OBS scene, and OBS is
told to make it the program:

```text
Main Camera        the scene called "Main Camera" goes to program
```

- **Settings.** `host` and `port`: the ones OBS shows under *Tools, WebSocket Server Settings* (the default is usually 4455, but PCHI does
  not assume it). A password, if OBS has one, goes in the environment variable `PCHI_OBS_PASSWORD` (preferred) or in the device's
  `password` parameter, which is **saved in the show file in clear text**. It is never sent to Studio or written to the record.
- **The link is the connection.** OBS keeps a WebSocket open, so the Live tab shows the device as linked, lost or silent, and the Doors
  check fails when OBS is not there. A dropped connection is remade every two seconds.
- **Cues are checked before the show.** An empty or over-long scene name fails the Doors check; Try a cue and Rehearse say which scene
  would be switched to. A scene OBS does not have is the cue's failure, with OBS's own reason ("No scene was found by the name of...").

**What is proven.** The protocol (hello, the salted double-SHA256 password proof, requests and responses) is as in the obs-websocket
documentation, and is tested against a fake OBS written from it, including a wrong and a missing password. It has **not** been run against a
real OBS: do that on the real-gear day (`real-gear-day.md`) and note the OBS version. Only scene switching is built: no recording,
streaming, sources, transitions or audio.

## Audience votes

A person opens a poll, the audience votes through a gateway you provide, and the show can wait for the result.

1. **Turn voting on:** start the runtime with `--audience-token <secret>` (or `PCHI_AUDIENCE_TOKEN`). It must differ from `--token` and
   `--edit-key`, because it is handed to an outside gateway.
2. **Open a poll:** on the Live tab (Audience polls), or `POST /polls/next/open` with `{"options": ["Calm", "Hype"]}`. This needs the right to
   run the show (an operator or a button); an agent cannot. Poll names are lower case letters, digits and `_`; 2 to 10 options.
3. **The audience votes:** your gateway (a web page, a chat bot) sends `POST /vote` with `{"poll": "next", "option": "hype", "voter": "ann"}`
   and the audience token (header `X-Pchi-Audience`, or `Authorization: Bearer`, or `?token=`). One vote per voter id per poll: a later
   vote replaces the earlier one.
4. **The show reads the room.** The tally is in the context under `vote`, and a guard can use it:

   ```text
   from intro to drop on go if vote.next.winner == hype
   ```

   `vote.next.hype` and `vote.next.calm` are the counts, `vote.next.total` all of them, `vote.next.winner` the leader's key (`""` while it is a
   tie or nobody has voted), `vote.next.open` true while it runs.
5. **Close it:** the result stays in the context, later votes are refused (409), and the result is a receipt (`poll.close`: who closed it, the
   counts, the winner).

**What the audience can do: vote, and nothing else.** The audience token opens `POST /vote` only. It cannot read the show, press GO, open or
close a poll, or edit anything, and the feed token cannot vote. Votes are capped at 300 a second and a poll takes at most 5000 voters.

**What it cannot do.** The runtime cannot tell whether voter ids are real people: the gateway that holds the token must give each person one
id, so preventing ballot stuffing is the gateway's job. Free-text chat is **not** accepted at all (it would need moderation), and a poll
decides nothing by itself: a person still presses GO, or the guard only unlocks a transition.
