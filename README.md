<p align="center">
    <a href="https://stalw.art">
    <img src="./img/logo-red.svg" height="150">
    </a>
</p>

<h3 align="center">
  Stalwart CLI
</h3>

<br>

<p align="center">
  <a href="https://github.com/stalwartlabs/cli/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/stalwartlabs/cli/release.yml?style=flat-square" alt="continuous integration"></a>
  &nbsp;
  <a href="https://www.gnu.org/licenses/agpl-3.0"><img src="https://img.shields.io/badge/License-AGPL_v3-blue.svg?label=license&style=flat-square" alt="License: AGPL v3"></a>
  &nbsp;
  <a href="https://stalw.art/docs/install/get-started"><img src="https://img.shields.io/badge/read_the-docs-red?style=flat-square" alt="Documentation"></a>
</p>
<p align="center">
  <a href="https://mastodon.social/@stalwartlabs"><img src="https://img.shields.io/mastodon/follow/109929667531941122?style=flat-square&logo=mastodon&color=%236364ff&label=Follow%20on%20Mastodon" alt="Mastodon"></a>
  &nbsp;
  <a href="https://twitter.com/stalwartlabs"><img src="https://img.shields.io/twitter/follow/stalwartlabs?style=flat-square&logo=x&label=Follow%20on%20Twitter" alt="Twitter"></a>
</p>
<p align="center">
  <a href="https://discord.com/servers/stalwart-923615863037390889"><img src="https://img.shields.io/discord/923615863037390889?label=Join%20Discord&logo=discord&style=flat-square" alt="Discord"></a>
  &nbsp;
  <a href="https://www.reddit.com/r/stalwartlabs/"><img src="https://img.shields.io/reddit/subreddit-subscribers/stalwartlabs?label=Join%20%2Fr%2Fstalwartlabs&logo=reddit&style=flat-square" alt="Reddit"></a>
</p>


A schema-driven command line tool for administering [Stalwart Mail and Collaboration Server](https://stalw.art) over its JMAP API.

The tool fetches the server's schema on first use and derives every command, validation rule, and rendered view from it. The same binary works against any compatible Stalwart deployment without recompilation.

## Overview

| | |
|---|---|
| `describe` | Inspect objects, fields, enums, filters, and sort options exposed by the server. |
| `get` / `query` | Fetch a single object or list / filter many. |
| `create` / `update` / `delete` | Single-object mutations. |
| `apply` | Apply a JSON plan of bulk creates, updates, and destroys (intended for Ansible, Terraform, NixOS, Pulumi, and CI/CD pipelines). |
| `snapshot` | Export live server state as an `apply`-ready JSON plan. Useful for backups, cross-environment promotion, and round-trip disaster-recovery rehearsals. |

Output is human-friendly by default (sectioned, with color when stdout is a TTY) and switches to compact JSON or NDJSON for machine consumption.

## Install

```sh
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/stalwartlabs/cli/releases/latest/download/stalwart-cli-installer.sh | sh

# Homebrew
brew install stalwartlabs/tap/stalwart-cli

# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/stalwartlabs/cli/releases/latest/download/stalwart-cli-installer.ps1 | iex"

# npm
npm install -g stalwart-cli

# From source
cargo install --path .
```

A signed `.msi` is also published with each release.

## Quick start

```sh
export STALWART_URL=https://mail.example.com
export STALWART_USER=admin
export STALWART_PASSWORD='changeme'

stalwart-cli describe                       # list every available object
stalwart-cli describe domain                # full schema for one object
stalwart-cli query domain                   # default columns
stalwart-cli get domain <id>                # full object
stalwart-cli create domain --field name=example.com --field isEnabled=true
stalwart-cli update domain <id> --field description='Primary'
stalwart-cli delete domain --ids <id>
stalwart-cli apply --file plan.json         # bulk apply
stalwart-cli snapshot Tenant Domain \        # export state as an apply plan
    --output backup.json
```

## Documentation

Full documentation, including the bulk-apply file format, JSON Schema, and integration guides for Ansible / Terraform / NixOS / Pulumi / CI:

**[stalw.art/docs/management/cli](https://stalw.art/docs/management/cli/overview)**

## License

This project is dual-licensed under the **GNU Affero General Public License v3.0** (AGPL-3.0; as published by the Free Software Foundation) and the **Stalwart Enterprise License v1 (SELv1)**:

- The [GNU Affero General Public License v3.0](./LICENSES/AGPL-3.0-only.txt) is a free software license that ensures your freedom to use, modify, and distribute the software, with the condition that any modified versions of the software must also be distributed under the same license. 
- The [Stalwart Enterprise License v1 (SELv1)](./LICENSES/LicenseRef-SEL.txt) is a proprietary license designed for commercial use. It offers additional features and greater flexibility for businesses that do not wish to comply with the AGPL-3.0 license requirements. 

Each file in this project contains a license notice at the top, indicating the applicable license(s). The license notice follows the [REUSE guidelines](https://reuse.software/) to ensure clarity and consistency. The full text of each license is available in the [LICENSES](./LICENSES/) directory.

## Copyright

Copyright (C) 2020, Stalwart Labs LLC
