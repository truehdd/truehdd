# Contributing

This file is for anyone sending a change, including agents. Most of it is about length,
because the usual problem with a generated patch is not that it is wrong. It is that the
code is fine and everything written around it is three times longer than it needs to be.

## The workspace

`truehd` is the library: extraction, parsing, decoding, conformance checks. The root crate
`truehdd` is the CLI: input, the threaded pipeline, DAMF, CAF and Wave64 output, `verify`
reporting. `truehdd-macros` serves the CAF writer only. Object audio metadata lives in the
separate [`oamd`](https://github.com/truehdd/oamd) crate.

Each of the three has its own changelog and its own version line, and is released on its own.

MSRV is 1.88 for the library and the macros and 1.95 for the CLI, checked with
`cargo +1.88 check -p truehd -p truehdd-macros` and `cargo +1.95 check`. Never nightly.

## Before you open a pull request

```bash
cargo test --workspace --all-features && cargo clippy --workspace --all-features -- -D warnings && cargo fmt --check
```

A fix needs a test that **fails without it**. Write the test, undo your change in the working
tree, watch the test fail, put it back. A test that passes either way documents nothing. Say in
the pull request what it prints when it fails, so a reviewer can confirm it without guessing.

A refactor or a move needs the opposite: decoded output must be **byte-identical** before and
after, across the fixtures and every `--presentation` selection. A move that changes a byte is
not a move.

## Commit messages

Semantic subject with a scope, `!` before the parens when it breaks:

```
fix(truehd): discard a timestamp with the candidate it belonged to
feat(cli): add the verify subcommand
refactor!(truehd): take the object audio metadata from the oamd crate
```

The body says **why**, in a few short paragraphs at most. It is not a walk through the diff,
a list of the files touched, or a record of what you tried. The reader can see the diff. What
they cannot see is the reason, so write that and stop.

Subject under 72 characters and shorter where it can be, body wrapped at 72.

Each commit builds and passes the tests on its own, and **no commit introduces something a
later commit in the same series removes or fixes**. If review finds a problem in an early
commit, that is where the fix belongs. Fixup commits are fine while review is under way; tidy
the series once, at the end.

Never invent commit metadata. Author dates are when the work happened, not a tidy ladder.

No `Co-Authored-By` lines for tools, and no "generated with" trailers.

## Changelog entries

Two or three sentences in the changelog of the crate that changed. Name the public item, say
what changed, say what a consumer does differently. Nothing about how it works inside. Mark a
breaking change `**BREAKING**:`.

A change that reaches users of both crates gets an entry in each, written for that crate's
reader: what the library gained, and separately what the CLI now does differently.

## Code comments

Only what the code cannot say itself. A comment earning its place usually explains a constraint,
a reason for an unobvious choice, or a trap the next reader would otherwise fall into. Restating
the line below it is worse than nothing.

The comment that always earns its place here is one line naming where a constant comes from,
next to the constant. A table index, a field width, a reserved value: say which rule fixes it.
Those are what a reviewer checks.

Never commit a comment that names a local path, a private test file, or where you got a fact
from a proprietary binary. This repository is public, and source files reach crates.io with the
release: a comment is published, not just committed.

## Pull request descriptions

Say what was wrong, what it does now, and how you know. Evidence is welcome and a table of
measurements is welcome: a one-line fix can need real reproduction context, and length spent on
evidence is length well spent.

What is not welcome is the same explanation three times. Decide where each thing belongs, then
do not repeat it: the reason goes in the commit body, the effect on a user goes in the
changelog, the evidence goes in the pull request.

## Review

Every change is verified, not taken on its description. Expect the reviewer to reproduce the
bug, run your test against unfixed code, and compare decoded output. Make that easy: say which
fixture shows the problem and what it prints.

That cuts both ways. Check a review comment before acting on it, including one from a tool. A
generated review is usually right about arithmetic and ranges, and often wrong about how much
something matters.
