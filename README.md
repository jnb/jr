# Jujusu Review

`jr` (Jujutsu Review) is a Rust-based CLI tool for translating Jujutsu commits
onto stacked GitHub PRs.

It's inspired by the Phabraciator Arcanist (`arc`) CLI tool, but tailored
towards Jujutsu and GitHub.

## Installation

Clone this repo and run:

```sh
cargo install --path .
```

By default, cargo install places binaries in `~/.cargo/bin/`. Make sure this
directory is in your PATH:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

## Quickstart

TODO

## Design principles

### History is preserved

Preserving history is the raison d'être for this tool.  If we instead force-push
our changes, using `jj git push`, then the GitHub review experience really
degrades:

- There's no way for reviewers to only see changes since their last review.
- Review comments are dropped / duplicated / moved; Github has to use heuristics
  to map old review comments onto the new branch.  These heuristics don't always
  work.

### Reviewers shouldn't know that we're using Jujutsu

To avoid confusing reviewers, they shouldn't be aware that we're using Jujutsu.
Instead, they should think "Yes, I can see how to do all of this using standard
Git practices.  But boy oh boy, it looks like it's a lot of work keeping all of
these branches synchronized.  I'll stick to using uber branches."

In more detail:

- Each Jujutsu commit is mapped onto a *single* remote Git branch, using the
  remote branch of the previous Jujutsu's commit as its base.  (This is in
  contrast to [Super Pull Requests](https://github.com/spacedentist/spr), where
  each PR has its own corresponding base branch.)
- Changes to an existing Jujutsu commit should appear as a new commit on the
  remote Git branch, with a suitable comment.
- Changes to base branches should be incorporated into the current branch using
  a merge commit.

### One Jujutsu commit per PR

This is what I use.
