/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use clap::{ArgGroup, Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "stalwart-cli",
    version,
    about = "Stalwart Command Line Interface",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Debug, Clone)]
pub struct GlobalArgs {
    /// Server endpoint (e.g. https://mail.example.com)
    #[arg(long, global = true, env = "STALWART_URL")]
    pub url: Option<String>,

    /// Basic-auth username
    #[arg(long, global = true, env = "STALWART_USER")]
    pub user: Option<String>,

    /// Basic-auth password (prompted if absent and stdin is a TTY)
    #[arg(long, global = true, env = "STALWART_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,

    /// Bearer token (mutually exclusive with --user)
    #[arg(
        long = "api-key",
        global = true,
        env = "STALWART_TOKEN",
        hide_env_values = true
    )]
    pub api_key: Option<String>,

    /// Skip TLS certificate verification
    #[arg(long, short = 'k', global = true)]
    pub insecure: bool,

    /// Disable ANSI color output
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Fetch a single object by id
    Get(GetArgs),
    /// Query objects with optional filters
    Query(QueryArgs),
    /// Create an object
    Create(CreateArgs),
    /// Update an object by id
    Update(UpdateArgs),
    /// Delete one or more objects by id
    Delete(DeleteArgs),
    /// Describe objects and enums from the schema
    Describe(DescribeArgs),
    /// Apply a bulk plan of creates, updates, and destroys from a JSON file
    Apply(ApplyArgs),
    /// Snapshot one or more object types into a plan file consumable by `apply`
    Snapshot(SnapshotArgs),
}

#[derive(Args, Debug)]
pub struct GetArgs {
    /// Object name (with or without the x: prefix)
    pub object: String,
    /// Object id (omit for singletons; literal "singleton" is also accepted)
    pub id: Option<String>,
    /// Comma-separated list of properties to return (omit to fetch all)
    #[arg(long, value_delimiter = ',')]
    pub fields: Option<Vec<String>>,
    /// Output JSON instead of human-readable text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct QueryArgs {
    pub object: String,
    /// Filter (repeatable). Syntax: field=value, field>=value, field<=value, field>value, field<value
    #[arg(long = "where", value_name = "FILTER")]
    pub wheres: Vec<String>,
    /// Comma-separated list of properties to return in each result
    #[arg(long, value_delimiter = ',')]
    pub fields: Option<Vec<String>>,
    /// Output JSON instead of human-readable text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("create_input").args(["field", "json", "file", "stdin"]).multiple(true)))]
pub struct CreateArgs {
    /// Object name; for multi-variant objects, use Object/Variant to select the variant
    pub object: String,
    /// Set a field. Repeatable. Value may be JSON if it starts with `{` or `[`.
    #[arg(long = "field", value_name = "KEY=VALUE")]
    pub field: Vec<String>,
    /// JSON object literal for the whole payload
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    /// Read JSON payload from a file
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,
    /// Read JSON payload from stdin
    #[arg(long)]
    pub stdin: bool,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("update_input").args(["field", "json", "file", "stdin"]).multiple(true)))]
pub struct UpdateArgs {
    pub object: String,
    /// Object id (omit for singletons; literal "singleton" is also accepted)
    pub id: Option<String>,
    /// Set a field (top-level name or JSON pointer path like `aliases/2/name`). Repeatable.
    #[arg(long = "field", value_name = "KEY=VALUE")]
    pub field: Vec<String>,
    /// JSON patch object literal
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    /// Read JSON patch from a file
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,
    /// Read JSON patch from stdin
    #[arg(long)]
    pub stdin: bool,
}

#[derive(Args, Debug)]
pub struct DeleteArgs {
    pub object: String,
    /// Comma-separated list of ids
    #[arg(long, value_name = "ID[,ID]*")]
    pub ids: Option<String>,
    /// Read ids from stdin (JSON array or comma/space/newline-separated)
    #[arg(long)]
    pub stdin: bool,
}

#[derive(Args, Debug)]
pub struct DescribeArgs {
    /// Object or enum name (with or without x: prefix). Omit to list all objects.
    pub name: Option<String>,
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("apply_input")
        .args(["file", "stdin"])
        .required(true)
        .multiple(false)
))]
pub struct ApplyArgs {
    /// Path to the JSON plan file
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,
    /// Read the JSON plan from stdin
    #[arg(long)]
    pub stdin: bool,

    /// Parse and validate the plan without calling the server
    #[arg(long)]
    pub dry_run: bool,
    /// Keep going after operation failures; report all errors at the end
    #[arg(long)]
    pub continue_on_error: bool,
    /// Suppress per-operation log lines; print only the final summary
    #[arg(long)]
    pub quiet: bool,
    /// Emit one NDJSON record per completed operation to stdout
    #[arg(long)]
    pub json: bool,
    /// Print per-batch progress during large destroys and creates
    #[arg(long)]
    pub progress: bool,
}

#[derive(Args, Debug)]
pub struct SnapshotArgs {
    /// Object types to include (positional, at least one). Use bare object
    /// names (`Domain`, `Account`, ...). For multi-variant types, all variants
    /// are included. View / variant slash forms are rejected.
    #[arg(required = true, value_name = "OBJECT")]
    pub objects: Vec<String>,

    /// Write the plan to this path instead of stdout
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Skip the destroy block at the top of the plan
    #[arg(long)]
    pub no_destroys: bool,

    /// Include secret field values as returned by the server (default: strip)
    #[arg(long)]
    pub include_secrets: bool,

    /// Comma-separated types whose references may be left unresolved. Any
    /// reference to one of these types found in the data is dropped from the
    /// exported plan.
    #[arg(long, value_name = "TYPES", value_delimiter = ',')]
    pub allow_unresolved: Vec<String>,

    /// Suppress progress output on stderr
    #[arg(long)]
    pub quiet: bool,
}
