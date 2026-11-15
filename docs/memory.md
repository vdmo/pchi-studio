# Agent memory

An agent that helps run a show needs to remember what happened last night and what people decided. It also needs to know when it does not know. This is that memory: local, small, and built to be wrong safely.

```bash
pchi-studio-api show.pchi --db show.db --memory memory.db        # the runtime serves it
pchi-memory --db memory.db derive run.jsonl                        # or from the command line: drafts from a recording
pchi-memory --db memory.db recall "why was the haze cue late"
```

## The rules

- **Decisions and evidence, not conversations.** A memory is a short statement (a sentence or a few) with a kind, a source, the hash of the log records it came from, and a status. Chat is not stored.
- **Typed, and it fades.** Each kind has a half-life: a *convention* (how this show likes things) 35 days, a *venue* fact 60, an *incident* 21, a *decision* 7, a *preference* 4, a *task* 23 hours. Standing falls along that curve from when it was written or verified; `sweep` archives what has fallen below 5 %. A person can **pin** a memory so it never fades.
- **Draft until a person verifies it.** Whatever an agent writes, and whatever is derived from a run, is a *draft*: it can be recalled, counts for 0.55 of a verified one, and is shown as `trust: draft`. Only a person (the operator or an editor) can verify, settle, supersede, pin or delete. An agent that asks for `verify` gets a draft.
- **Disagreement is kept, not merged.** Two memories under the same `key` that say different things are both stored and marked in conflict. When they are close, recall will not choose: it answers that they disagree. A person **settles** it (this one is right; the other is archived and points to it) or **supersedes** one with new words.
- **Recall can say "I don't know".** It declines when nothing is remembered, when nothing remembered is about the question, when what is about it is too old or too uncertain, or when two memories disagree. The answer carries `answer: false`, the reason, and what came closest, so a person can look. An agent should relay that, not fill the gap.
- **Being recalled is not being true.** How often a memory is recalled adds a small, bounded bonus to its ranking (at most 0.1, scaled by its status). It never changes its status.
- **Everything is audited.** Each write, recall, verification, settlement, expiry and deletion is a row in an append-only audit table (`pchi-memory audit`, `export`). Verifying, settling, superseding and deriving through the runtime also leave a signed receipt.
- **Deleting fails closed.** It needs a person *and* their explicit request (`--yes`).
- **Never in the show's path.** No cue, timer, takeover or lasers touch memory. If the file cannot be read the show does not notice.

## Where memory comes from

- **Derived from the run.** `POST /memory/derive` (or `pchi-memory derive <recording>`) writes drafts from what the log says: cues to a device that did not go through (one per device and reason), the three cues that ran furthest late, restarts, takeovers and pauses, and decisions with who and why (a rejected proposal, an edit allowed or refused). Each carries the hash of the records it came from, and ids are deterministic, so deriving a run twice changes nothing and `check_evidence` can tell a memory whose records have since changed. Nothing is invented: each memory is a sentence built from recorded fields.
- **Written by an agent** (`pchi_memory_remember` over MCP, or `POST /memory`): a draft under the agent's name.
- **Written by a person**, optionally verified at once.

## Asking

| | |
|---|---|
| `GET /memory?q=…&kind=…&limit=…` | ask. Anyone who passes the token may |
| `GET /memory/list?status=draft` | what is stored |
| `POST /memory` `{text, kind, key, dept, verify}` | write (agents: always a draft) |
| `POST /memory/<id>/verify` | a person says it is right |
| `POST /memory/<id>/settle` `{over}` | this one is right, the other is wrong |
| `POST /memory/<id>/supersede` `{text}` | replace with new words |
| `POST /memory/derive` | drafts from this run |

MCP (`pchi-mcp`): `pchi_memory_recall` and `pchi_memory_remember`. An agent can ask and write drafts and nothing more; the runtime enforces that.

## How it searches, and how well

Local, no model, no network: **BM25** keyword search, a **hashed character-trigram vector** (a cheap stand-in that catches typos and endings such as `venu netwrk`; it is *not* a neural embedding and does not know that "pyro" and "fireworks" mean the same), and the two **fused**. Whether to answer is decided separately, from how much of the question's content the memory has, weighting each word by how rare it is among the memories.

`pchi-memory eval` measures all of it against a **golden set**: memories (each with a key) and questions saying which key should come back, or that nothing should. It reports recall@1, @3, @5 and MRR for each way of searching, and what the shipped recall did: answered correctly, answered with the **wrong** memory (the worst outcome), declined although there was an answer, kept quiet on what it should, or answered anyway. `--fail-under-recall5 0.8` and `--max-wrong 0` make it a gate you can fail a build on. The idea (a recall gate and a strategy comparison at no API cost) is the one `pgrag-eval-lite` uses for document chunks; this is the same discipline for a show's memory.

The sample that ships (`crates/pchi-memory/golden/sample.json`: 20 memories, 30 questions) gave:

| Search | recall@1 | recall@5 |
|---|---|---|
| keyword | 84 % | 84 % |
| trigram vector | 88 % | 88 % |
| hybrid | 88 % | 88 % |

Shipped recall: 22 of 25 answerable questions answered correctly, **0 wrong**, 3 declined; all 5 unanswerable questions declined. The three it could not answer share no word with their answer ("smoke effect" for a memory about haze, "beams" for lasers, "confetti" for a pyrotechnics ban): a keyword and trigram search cannot find those, and declining is the right way to fail.

**Read that with care.** The sample is synthetic, written by us, and small. The abstention threshold was first set at 0.25, which answered three questions about things never written down ("who is the lighting designer" with the lighting convention); it was changed to 0.45 and the rule to weight words by rarity *after seeing that*, so the sample no longer tests it fairly. It says the machinery works; it does not say how it will do on your shows. Write a golden set from your own recordings and questions, and run it before you rely on recall.

## In Studio

Live tab, **Memory**: ask a question (an answer shows each note as *verified* or *draft: not yet verified*; when memory does not know it says so, with the reason and what came closest, marked as not an answer); **Derive from this run**; **Waiting for a person**, the drafts, each with **Verify** and **Archive**; **Memories that disagree**, shown together, with **This one is right** on each (the other is archived and points to it); and the verified notes. The panel only asks: the runtime decides who may verify, archive or settle (an operator or an editor), and says so in words when someone else tries.

The hand-over page (`docs/report.md`) has a **Memory from this run** section: how many drafts the run left waiting, how many are verified, how many more could be derived, and the drafts themselves, so the review after a show is one routine: derive, read the page, verify what is worth keeping.

## Writing a golden set from your own shows

The sample that ships tells you the machinery works. It cannot tell you how recall does on *your* questions, and it is mostly easy: its own report says 21 of its 25 questions share most of their words with the answer. To find out for real:

```bash
pchi-memory golden-init run1.jsonl run2.jsonl --out golden.json   # the notes your real runs give rise to, as m1, m2, ...
# write the questions yourself, the way a person would ask them, in other words than the notes use
pchi-memory golden-check golden.json                              # what makes it say little
pchi-memory eval golden.json --save-baseline base.json            # the numbers, and a baseline to hold later changes to
pchi-memory eval golden.json --baseline base.json --max-wrong 0   # after any change: fails if recall fell or a wrong answer appeared
```

`golden-check` and the top of every `eval` report say when the set is too small (under 30 answerable questions: one question moves recall by 3 or more points), has fewer than 5 questions memory should decline, has notes no question expects, repeats the same question, or has questions that repeat almost every word of their answer (they test nothing a keyword search would not). `eval` also reports recall **by how much of the question's wording the right note shares**. That table is the one that answers whether a real embedding model is worth adding: it would have to beat the trigram search in the "shares few or none" row, and there is no point in adding one before you have enough questions in that row to tell. The three such questions in the shipped sample are found by no search, so the sample cannot say.

## What it does not do

- It does not understand meaning. Words that mean the same but look different are missed (the trigram search is not an embedding; a real one can be put behind the same interface, and `eval` is how you would know if it helps).
- It does not decide what is *true*, only who vouched for it and how long ago. A verified memory can be wrong.
- It is not shared: each runtime has its own file, a standby has its own, and nothing syncs between shows or venues. After a takeover run `POST /memory/derive` on the standby. Sharing across shows is a decision about whose notes go where, and has not been built.
- The derived memories are only the kinds listed. A run's meaning is not extracted.
- Studio's screen (below) is plain: ask, review, verify. It has no search of the verified notes, no editing of a note's words (supersede is the command or the route), and no pinning.
