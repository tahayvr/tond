# Reporting vulnerabilities

If you find a security problem in Tond, please report it privately so it can be fixed before it is public. This page explains where to send it, what to include, and which versions receive fixes.

## How to report

Use GitHub's private vulnerability reporting:

1. Go to [github.com/tahayvr/tond](https://github.com/tahayvr/tond).
2. Open the **Security** tab.
3. Click **Report a vulnerability** and fill in the form.

The report is visible only to the maintainers. You can discuss it and follow the fix in the same private advisory.

> [!CAUTION]
> Do not open a public issue, pull request or discussion for a security problem, and do not post details elsewhere until a fix is released.

## What to include

A good report lets the problem be reproduced quickly:

- The Tond version (`cat /opt/tond/VERSION`) and the CPU architecture.
- The Linux distribution and version, and the Docker version.
- Which part is affected: `tond-server`, `tond-proxy`, the dashboard, the CLI, the installer or update script, or a release artifact.
- What an attacker needs to start with (no access, a viewer account, a scoped API token, a deployed app, a pull request that triggers a preview) and what they gain.
- Step-by-step instructions, a proof of concept, or a request that shows the problem.
- Any logs that help (`journalctl -u tond-server`, `journalctl -u tond-proxy`), with secrets removed.
- Whether you plan to publish your findings, and when.

The [threat model](/security/threat-model) describes what Tond aims to defend against and what is out of scope. A report is welcome even if you are not sure it counts.

## Please do not

- Test against servers you do not own or have permission to test.
- Access, change or delete other people's data.
- Run denial-of-service tests against shared infrastructure.

## Supported versions

Security fixes are made for the latest release only. Update to it before reporting if you can, and check whether the problem still occurs. See [Upgrading Tond](/guides/upgrading).

Releases are signed. The installer, `update.sh` and `tond update` verify a release's signature before installing it, so a fixed release reaches you intact.
