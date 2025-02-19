# mergelog

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

Here's the full `--help` output:

```
Usage: mergelog <changelog_directory> [--repo <repo>] [--host <host>] [-s <section...>]

Merges changelog files into a single changelog

Positional Arguments:
  changelog_directory
                    directory containing changelogs and a mergelog.toml

Options:
  --repo            link to the repository to resolve merge/pull requests at;
                    omit to infer from the current repo
  --host            the repository host; omit to infer from the repo URL
  -s, --section     changelog sections in order
  --help, help      display usage information
```
