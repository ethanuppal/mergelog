# mergelog

[![Code Style Badge](https://github.com/ethanuppal/mergelog/actions/workflows/lint.yaml/badge.svg)](https://github.com/ethanuppal/mergelog/blob/main/.github/workflows/lint.yaml)
[![cargo-deny badge](https://github.com/ethanuppal/mergelog/actions/workflows/cargo-deny.yaml/badge.svg)](https://github.com/ethanuppal/mergelog/blob/main/.github/workflows/cargo-deny.yaml)
[![Crates.io Version](https://img.shields.io/crates/v/mergelog)](https://crates.io/crates/mergelog)
[![Crates.io License](https://img.shields.io/crates/l/mergelog)](./LICENSE)

> [!CAUTION]
> `mergelog` only supports GitLab now, but adding GitHub should be trivial.

`mergelog` is a simple tool to combine changelog entries spread over multiple
files into one, interactively inferring and resolving corresponding pull
requests.

To get started, just run:

```bash
cargo install mergelog
mergelog my/changelog/directory
```

I'm demoing it on [Spade](http://gitlab.com/spade-lang/spade), a programming
langauge I contribute to:

https://github.com/user-attachments/assets/9d8bef51-0a6d-420e-860d-812dd872be87

Here's the full `--help` output:

```
Usage: mergelog <changelog_directory> [--repo <repo>] [--host <host>] [-s <section...>] [--link-at-start]

Merges changelog files into a single changelog

Positional Arguments:
  changelog_directory
                    directory containing changelogs and a mergelog.toml

Options:
  --repo            link to the repository to resolve merge/pull requests at;
                    omit to infer from the current repo
  --host            the repository host; omit to infer from the repo URL
  -s, --section     changelog sections in order
  --link-at-start   whether the link to pull requests in the changelog should be
                    put at the start of each entry
  --help, help      display usage information
```
