# Ableton Link (listen only)

`pchi-studio-api --link-peers 5` listens for five seconds (1 to 60) for Ableton Link peers on this network and prints each one: its identity, session, tempo, and whether it is playing. Then it exits.

```
peer 4142434445464748 session 0909090909090909 tempo 120.00 bpm playing true from 192.168.1.20:57104
```

## What it is, and is not

- **Listen only.** It joins the Link multicast group (224.76.78.75, port 20808) and reads. It sends nothing, never joins a session, and never changes anyone's tempo or start/stop.
- **It does not drive the show.** The runtime has no tempo of its own, so there is nothing for a Link tempo to feed yet. This is a way to see that Link peers are present and what tempo they report, for example before a show.
- **Not beat or phase sync.** Following Link's beat position needs the clock measurement exchange (ping/pong with each peer), which is not built.
- The port is shared, so this works next to an app that already runs Link on the same machine.
- At most 64 peers are remembered, so a stray sender cannot grow memory by inventing identities. A "bye" message removes a peer. Datagrams that do not match the layout are dropped, not guessed at.

## Basis

The byte layout was read from the published Link source (Ableton/link, GPL v2 or later) on 2026-11-19: the message header, the entry framing, the `tmln` timeline (tempo as microseconds per beat), `sess` and `stst` entries, and the multicast address and port. `link.rs` is a fresh reading of that layout, not a copy of the code. Link's own licence terms apply if you ever embed Link itself; this reads the wire format only.

## What was tested

- Exact bytes of an alive message (header, type, ttl, group, ident, each entry's key and size), and a read-back.
- Unknown entries skipped; short, oversized, wrong-version, wrong-size, zero-tempo and negative-tempo datagrams refused.
- A listener over a loopback socket: peers listed, a bye removes one, garbage ignored, the peer cap holds.
- One real multicast datagram sent to 224.76.78.75:20808 on a WSL machine and read by `--link-peers`: it printed 120.00 bpm.

## What was not

- **No real Link peer was ever heard.** Every test datagram was built from the same reading of the source, so this shows the decoder matches that reading, not that Live or any other app sends exactly that. The first thing to do with real gear is run `--link-peers` next to a Link-enabled app.
- Only IPv4. Only `tmln`, `sess` and `stst` are read; the endpoint entries (`mep4`, `aep4` and the IPv6 ones) are skipped. Interfaces are not chosen: the group is joined on the default one.
- Newer Link versions may add entries or change behaviour; the source read was the `master` branch on the date above.
