use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use crate::commands::{
    self, AddInput, AdoptInput, FormatSelection, ListInput, RemoveInput, SearchInput, UpdateInput,
};
use crate::error::Result;
use crate::model::FormatKind;
use crate::paths::AppPaths;

#[derive(Debug, Parser)]
#[command(name = "lmm")]
#[command(version)]
#[command(about = "Local AI model manager — download once, use everywhere, remove cleanly")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Install a Hugging Face model artifact and expose it to local tools")]
    #[command(alias = "a", alias = "i", alias = "install")]
    Add(AddArgs),
    #[command(about = "Remove model exposures and reclaim managed cache bytes")]
    #[command(alias = "rm")]
    Remove(RemoveArgs),
    #[command(about = "List all local model artifacts")]
    #[command(alias = "ls")]
    List(ListArgs),
    #[command(about = "Adopt untracked HF Cache models into lmm management")]
    Adopt(AdoptArgs),
    #[command(about = "Show details for one tracked model")]
    Info(InfoArgs),
    #[command(about = "Validate tracked models, cache files, and exposures")]
    Doctor(DoctorArgs),
    #[command(about = "Clean temporary files, stale exposure records, and orphan cache blobs")]
    Gc(GcArgs),
    #[command(about = "Search Hugging Face models (interactive without query)")]
    #[command(alias = "find", alias = "discover")]
    Search(SearchArgs),
    #[command(about = "Update tracked models to the latest commit for their revision")]
    #[command(alias = "upgrade")]
    Update(UpdateArgs),
    #[command(about = "Read or update configuration")]
    Config(ConfigCommand),
    #[command(about = "Generate shell completions")]
    Completions(CompletionsArgs),
}

#[derive(Debug, Args)]
struct AddArgs {
    #[arg(help = "Hugging Face repo id, for example mlx-community/Qwen3-8B-4bit")]
    repo: Option<String>,
    #[arg(
        long,
        default_value = "main",
        help = "HF revision, branch, tag, or commit"
    )]
    revision: String,
    #[arg(
        long,
        help = "Specific file or wildcard for GGUF selection (e.g. *Q4_K_M*)"
    )]
    file: Option<String>,
    #[arg(long, help = "Model alias stored in the lock file")]
    name: Option<String>,
    #[arg(
        long = "tool",
        value_delimiter = ',',
        help = "Comma-separated target tools"
    )]
    tools: Vec<String>,
    #[arg(long, value_enum, default_value_t = FormatArg::Auto, help = "Artifact format")]
    format: FormatArg,
    #[arg(long, help = "List supported artifacts in the repo without installing")]
    list: bool,
    #[arg(long, help = "Expose to all compatible dedupe-preserving tools")]
    all: bool,
    #[arg(long, help = "Print the install plan without changing files")]
    dry_run: bool,
    #[arg(long, help = "Replace an existing alias or tool exposure")]
    replace: bool,
    #[arg(long, help = "Take ownership of an already-present HF Cache artifact")]
    take_ownership: bool,
    #[arg(
        long,
        help = "Reserved for future Ollama integration (currently unused)"
    )]
    ollama_name: Option<String>,
    #[arg(short = 'y', long, help = "Skip confirmation prompts")]
    yes: bool,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    #[arg(help = "Model aliases or leftover exposure names to remove")]
    names: Vec<String>,
    #[arg(
        long = "tool",
        value_delimiter = ',',
        help = "Only remove exposures for these tools"
    )]
    tools: Vec<String>,
    #[arg(
        long,
        help = "Remove every matching tracked model and leftover exposure"
    )]
    all: bool,
    #[arg(long, help = "Remove tool exposures but keep HF Cache bytes")]
    keep_cache: bool,
    #[arg(long, help = "Also purge adopted HF Cache bytes")]
    purge_cache: bool,
    #[arg(long, help = "Print the removal plan without changing files")]
    dry_run: bool,
    #[arg(short = 'y', long, help = "Skip confirmation prompts")]
    yes: bool,
}

#[derive(Debug, Args)]
struct ListArgs {
    #[arg(long, help = "Show repo and commit details")]
    wide: bool,
    #[arg(long, help = "Show canonical and exposure paths")]
    paths: bool,
    #[arg(long, help = "Emit JSON")]
    json: bool,
    #[arg(long, value_enum, help = "Filter by artifact format")]
    format: Option<FormatArg>,
    #[arg(long, help = "Filter by exposure tool")]
    tool: Option<String>,
}

#[derive(Debug, Args)]
struct AdoptArgs {
    #[arg(help = "Model names to adopt (omit for all)")]
    names: Vec<String>,
    #[arg(
        long = "tool",
        value_delimiter = ',',
        help = "Expose adopted models to these tools"
    )]
    tools: Vec<String>,
    #[arg(long, help = "Make adopted artifacts removable by lmm remove")]
    take_ownership: bool,
    #[arg(short = 'y', long, help = "Skip confirmation prompts")]
    yes: bool,
}

#[derive(Debug, Args)]
struct InfoArgs {
    #[arg(help = "Tracked model alias")]
    name: String,
    #[arg(long, help = "Emit JSON")]
    json: bool,
    #[arg(long, help = "Show artifact files")]
    files: bool,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    #[arg(long, help = "Mark stale exposure records in the lock file")]
    fix: bool,
    #[arg(long, help = "Check blob sizes and orphan cache blobs")]
    deep: bool,
}

#[derive(Debug, Args)]
struct GcArgs {
    #[arg(long, help = "Include adopted artifacts in cleanup")]
    include_adopted: bool,
    #[arg(short = 'y', long, help = "Execute the cleanup (default: dry-run)")]
    yes: bool,
}

#[derive(Debug, Args)]
struct SearchArgs {
    #[arg(help = "Hugging Face search query")]
    query: Option<String>,
    #[arg(long, value_enum, default_value_t = FormatArg::Auto, help = "Filter by artifact format")]
    format: FormatArg,
    #[arg(long, help = "Filter by HF author or organization")]
    author: Option<String>,
    #[arg(long, default_value_t = 100, help = "Maximum results")]
    limit: usize,
    #[arg(long, default_value = "downloads", help = "HF API sort field")]
    sort: String,
    #[arg(long, help = "Skip prompts after selecting a model")]
    yes: bool,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    #[arg(help = "Tracked model aliases to update")]
    names: Vec<String>,
    #[arg(long, help = "Update every tracked model")]
    all: bool,
    #[arg(long, help = "Print the update plan without changing files")]
    dry_run: bool,
    #[arg(short = 'y', long, help = "Skip confirmation prompts")]
    yes: bool,
}

#[derive(Debug, Subcommand)]
enum ConfigSubcommand {
    #[command(about = "Print one config value")]
    Get { key: String },
    #[command(about = "Set one config value")]
    Set { key: String, value: String },
    #[command(about = "Print the config file path")]
    Path,
}

#[derive(Debug, Args)]
struct ConfigCommand {
    #[command(subcommand)]
    command: ConfigSubcommand,
}

#[derive(Debug, Args)]
struct CompletionsArgs {
    #[arg(help = "Shell to generate completions for")]
    shell: Shell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum FormatArg {
    Auto,
    Mlx,
    Gguf,
    Safetensors,
}

impl From<FormatArg> for FormatSelection {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Auto => Self::Auto,
            FormatArg::Mlx => Self::Format(FormatKind::Mlx),
            FormatArg::Gguf => Self::Format(FormatKind::Gguf),
            FormatArg::Safetensors => Self::Format(FormatKind::Safetensors),
        }
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::resolve()?;
    let config = crate::config::Config::load(&paths.config_path)?;
    crate::format::init_color(&config.ui.color);

    let Some(command) = cli.command else {
        crate::format::banner();
        return Ok(());
    };

    match command {
        Command::Add(args) => commands::add(
            &paths,
            &AddInput {
                repo: args.repo,
                revision: args.revision,
                file: args.file,
                name: args.name,
                tools: args.tools,
                format: args.format.into(),
                list: args.list,
                all: args.all,
                dry_run: args.dry_run,
                replace: args.replace,
                take_ownership: args.take_ownership,
                ollama_name: args.ollama_name,
                yes: args.yes,
            },
        ),
        Command::Remove(args) => commands::remove(
            &paths,
            &RemoveInput {
                names: args.names,
                tools: args.tools,
                all: args.all,
                keep_cache: args.keep_cache,
                purge_cache: args.purge_cache,
                dry_run: args.dry_run,
                yes: args.yes,
            },
        ),
        Command::List(args) => commands::list(
            &paths,
            &ListInput {
                wide: args.wide,
                paths: args.paths,
                json: args.json,
                format: args
                    .format
                    .map_or(FormatSelection::Auto, FormatSelection::from),
                tool: args.tool,
            },
        ),
        Command::Adopt(args) => commands::adopt(
            &paths,
            &AdoptInput {
                names: args.names,
                tools: args.tools,
                take_ownership: args.take_ownership,
                yes: args.yes,
            },
        ),
        Command::Info(args) => commands::info(&paths, &args.name, args.json, args.files),
        Command::Doctor(args) => commands::doctor(&paths, args.fix, args.deep),
        Command::Gc(args) => commands::gc(&paths, args.yes, args.include_adopted),
        Command::Search(args) => commands::search(
            &paths,
            &SearchInput {
                query: args.query,
                format: args.format.into(),
                author: args.author,
                limit: args.limit,
                sort: args.sort,
                yes: args.yes,
            },
        ),
        Command::Update(args) => commands::update(
            &paths,
            &UpdateInput {
                names: args.names,
                all: args.all,
                dry_run: args.dry_run,
                yes: args.yes,
            },
        ),
        Command::Config(config) => match config.command {
            ConfigSubcommand::Get { key } => commands::config_get(&paths, &key),
            ConfigSubcommand::Set { key, value } => commands::config_set(&paths, &key, &value),
            ConfigSubcommand::Path => {
                commands::config_path(&paths);
                Ok(())
            }
        },
        Command::Completions(args) => {
            commands::completions::<Cli>(args.shell);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_alias_should_parse_default_revision_and_tools() {
        let cli = Cli::try_parse_from([
            "lmm",
            "install",
            "org/repo",
            "--tool",
            "lmstudio,jan",
            "--format",
            "gguf",
            "--yes",
        ])
        .unwrap();

        let Some(Command::Add(args)) = cli.command else {
            panic!("expected add command");
        };
        assert_eq!(args.repo.as_deref(), Some("org/repo"));
        assert_eq!(args.revision, "main");
        assert_eq!(args.tools, vec!["lmstudio".to_string(), "jan".to_string()]);
        assert_eq!(
            FormatSelection::from(args.format),
            FormatSelection::Format(FormatKind::Gguf)
        );
        assert!(args.yes);
    }

    #[test]
    fn list_alias_should_parse_optional_format_filter() {
        let cli = Cli::try_parse_from(["lmm", "ls", "--format", "mlx", "--paths"]).unwrap();

        let Some(Command::List(args)) = cli.command else {
            panic!("expected list command");
        };
        assert_eq!(args.format, Some(FormatArg::Mlx));
        assert!(args.paths);
    }

    #[test]
    fn search_alias_should_parse_find_and_discover() {
        for alias in ["find", "discover"] {
            let cli = Cli::try_parse_from(["lmm", alias, "qwen", "--limit", "5"]).unwrap();
            let Some(Command::Search(args)) = cli.command else {
                panic!("expected search command via {alias}");
            };
            assert_eq!(args.query.as_deref(), Some("qwen"));
            assert_eq!(args.limit, 5);
        }
    }

    #[test]
    fn completions_should_parse_shell() {
        let cli = Cli::try_parse_from(["lmm", "completions", "zsh"]).unwrap();

        let Some(Command::Completions(args)) = cli.command else {
            panic!("expected completions command");
        };
        assert_eq!(args.shell, Shell::Zsh);
    }
}
