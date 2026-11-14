# Recording and replaying a run

The runtime writes every input, context change, cue, delivery, live edit and decision to its log. A **recording** is that log for the latest run, with the show it started
from, as one JSON file (`pchi-run/1`). Keep it with the post-show notes, send it to someone who was not there, or open it in Studio and drag through it.

## In Studio

Live tab → **Replay a run**.

- **Load this run** reads the run on the runtime now (a run in progress works too: it is as far as it has got).
- **Open a recording…** reads a file saved earlier. It never touches the running show.
- **Save recording** downloads the latest run as a file.

Then drag along the bar or click it, press **Play** (1× to 16×), or **Next moment**. The bar shows each scene as a coloured band, with marks for live edits (yellow), decisions
(white), deliveries that went wrong or inputs that were held (red) and a restart or takeover (orange). Under it: the scene the show was in, how long it had been there, the context
values at that moment, how many cues had gone, and everything that had happened by then, in order, with who decided what.

The line above the bar says whether the run **replays exactly**: the recorded inputs are played back through the show (with each live edit applied where it happened) and must
go where the recording says. A recording that does not replay was edited, or comes from a show that has since changed in a way that matters.

## From the command line

```bash
pchi-studio-api --db show.db --export-run run.json     # the latest run in a log, as a file
pchi-studio-api --replay run.json                      # print it as a timeline, and check it replays
pchi-studio-api new-version.pchi --rehearse new-version.pchi --recording run.json   # an edited show against that run
```

`--replay` needs no show file. Over HTTP: `GET /recording` (the file), `GET /replay` (the laid-out view, with the check), `POST /replay` with a recording as the body (the same for a
file; refused with 400 if it is not one).

## What is in a recording

Everything the log holds for the run: inputs and who sent them, context values, cue times (planned and actual), deliveries and their errors, live edits (the whole show after each),
decisions with the names from the roster and the reasons people and agents gave, and the rig's addresses. It holds **no keys, tokens or passwords** (a test checks this). Treat it
like the show file: it says who pressed what.

## What it is not

- It does not record what the tools did, only what the runtime sent and what came back.
- Replaying is reading. It does not re-send anything to a device. (A run can be tried against an edited show with `--rehearse`, see `rehearse.md`.)
- Times are the runtime's clock, in milliseconds from the start of the run.
