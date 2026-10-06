# Security policy

## Reporting a vulnerability
Please report security issues privately by email to **jendrik@madewithtea.com**. Do not open a public
GitHub issue, pull request or discussion for them.

If you can, encrypt your mail to the same key that signs the releases:

    A6E6 9ED6 BC47 79F3 2817  2148 4CC1 AFDE 15B6 4EA3
    gpg --keyserver hkps://keys.openpgp.org --recv-keys A6E69ED6BC4779F3281721484CC1AFDE15B64EA3

Please include:
- the opensidian version and package (slim / portable AppImage, Flatpak, source build)
- your distribution
- steps to reproduce, or a proof of concept (for example a vault, note or theme that triggers it)
- what an attacker gains

## What happens next
- You get an acknowledgement within 7 days.
- I confirm the issue, fix it and publish a signed release. You are credited in the release notes
  unless you'd rather not be.
- Please give me 90 days, or until the fix is released (whichever comes first), before disclosing
  publicly. If a fix needs longer, I'll tell you why and we agree on a date.

## Scope
In scope: the opensidian app and its release artifacts, including anything a malicious vault, note,
theme, CSS snippet or link can do (script execution, file access outside the vault, escaping the
Landlock or Flatpak sandbox).

Out of scope: bugs in the bundled third-party themes that don't affect the app's security (report
them upstream; see [THIRD-PARTY.md](THIRD-PARTY.md)), and issues that need an attacker who already
controls your user account.

## Supported versions
Only the latest release gets security fixes.

| Version | Supported          |
| ------- | ------------------ |
| v0.4    | :white_check_mark: |
| < v0.4  | :x:                |
