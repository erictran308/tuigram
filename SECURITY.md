# Security

## Reporting a vulnerability

Report it privately at <https://github.com/erictran308/tuigram/security/advisories/new> (Security → Report a vulnerability), not in a public issue: tuigram holds people's Telegram sessions, and a public report tells everyone how to reach them before a fix is out.

Say what someone has to send or do, what happens, your tuigram version (`tuigram --version`), OS and terminal.

## Supported versions

Only the latest release gets fixes.

## What counts

Anything another Telegram user, or a file, link or folder tuigram is pointed at, can do to you through tuigram. For example:

- a message, name, link, file or image that runs code, opens something without asking, or garbles the terminal;
- messages marked read, or you shown online, while you're away;
- your session or API key read, replaced or left where another account can reach it;
- files sent that you didn't pick.

Bugs in TDLib itself belong at <https://github.com/tdlib/td>.

## Checking a release

Release binaries are built only by this repository's release workflow, which attaches a build provenance attestation. To check an archive:

```sh
gh attestation verify tuigram-<target>.tar.gz --repo erictran308/tuigram
```
