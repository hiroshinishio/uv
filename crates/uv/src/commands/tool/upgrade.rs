use std::{collections::BTreeSet, fmt::Write};

use anyhow::Result;
use owo_colors::OwoColorize;
use tracing::debug;

use crate::commands::pip::loggers::{DefaultInstallLogger, DefaultResolveLogger};
use crate::commands::project::update_environment;
use crate::commands::tool::common::{remove_entrypoints, InstallAction};
use crate::commands::{tool::common::install_executables, ExitStatus, SharedState};
use crate::printer::Printer;
use crate::settings::ResolverInstallerSettings;
use uv_cache::Cache;
use uv_client::Connectivity;
use uv_configuration::{Concurrency, PreviewMode};
use uv_normalize::PackageName;
use uv_requirements::RequirementsSpecification;
use uv_settings::{Combine, ResolverInstallerOptions, ToolOptions};
use uv_tool::InstalledTools;
use uv_warnings::warn_user_once;

/// Upgrade a tool.
pub(crate) async fn upgrade(
    name: Option<PackageName>,
    connectivity: Connectivity,
    args: ResolverInstallerOptions,
    filesystem: ResolverInstallerOptions,
    concurrency: Concurrency,
    native_tls: bool,
    cache: &Cache,
    preview: PreviewMode,
    printer: Printer,
) -> Result<ExitStatus> {
    if preview.is_disabled() {
        warn_user_once!("`uv tool upgrade` is experimental and may change without warning");
    }

    // Initialize any shared state.
    let state = SharedState::default();

    let installed_tools = InstalledTools::from_settings()?.init()?;
    let _lock = installed_tools.acquire_lock()?;

    let names: BTreeSet<PackageName> =
        name.map(|name| BTreeSet::from_iter([name]))
            .unwrap_or_else(|| {
                installed_tools
                    .tools()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect()
            });

    if names.is_empty() {
        writeln!(printer.stderr(), "Nothing to upgrade")?;
        return Ok(ExitStatus::Success);
    }

    for name in names {
        debug!("Upgrading tool: `{name}`");

        // Ensure the tool is installed.
        let existing_tool_receipt = match installed_tools.get_tool_receipt(&name) {
            Ok(Some(receipt)) => receipt,
            Ok(None) => {
                let install_command = format!("uv tool install {name}");
                writeln!(
                    printer.stderr(),
                    "`{}` is not installed; run `{}` to install",
                    name.cyan(),
                    install_command.green()
                )?;
                return Ok(ExitStatus::Failure);
            }
            Err(_) => {
                let install_command = format!("uv tool install --force {name}");
                writeln!(
                    printer.stderr(),
                    "`{}` is missing a valid receipt; run `{}` to reinstall",
                    name.cyan(),
                    install_command.green()
                )?;
                return Ok(ExitStatus::Failure);
            }
        };

        let existing_environment = match installed_tools.get_environment(&name, cache) {
            Ok(Some(environment)) => environment,
            Ok(None) => {
                let install_command = format!("uv tool install {name}");
                writeln!(
                    printer.stderr(),
                    "`{}` is not installed; run `{}` to install",
                    name.cyan(),
                    install_command.green()
                )?;
                return Ok(ExitStatus::Failure);
            }
            Err(_) => {
                let install_command = format!("uv tool install --force {name}");
                writeln!(
                    printer.stderr(),
                    "`{}` is missing a valid environment; run `{}` to reinstall",
                    name.cyan(),
                    install_command.green()
                )?;
                return Ok(ExitStatus::Failure);
            }
        };

        // Resolve the appropriate settings, preferring: CLI > receipt > user.
        let options = args.clone().combine(
            ResolverInstallerOptions::from(existing_tool_receipt.options().clone())
                .combine(filesystem.clone()),
        );
        let settings = ResolverInstallerSettings::from(options.clone());

        // Resolve the requirements.
        let requirements = existing_tool_receipt.requirements();
        let spec = RequirementsSpecification::from_requirements(requirements.to_vec());

        // TODO(zanieb): Build the environment in the cache directory then copy into the tool
        // directory.
        let environment = update_environment(
            existing_environment,
            spec,
            &settings,
            &state,
            Box::new(DefaultResolveLogger),
            Box::new(DefaultInstallLogger),
            preview,
            connectivity,
            concurrency,
            native_tls,
            cache,
            printer,
        )
        .await?;

        // At this point, we updated the existing environment, so we should remove any of its
        // existing executables.
        remove_entrypoints(&existing_tool_receipt);

        install_executables(
            &environment,
            &name,
            &installed_tools,
            ToolOptions::from(options),
            true,
            existing_tool_receipt.python().to_owned(),
            requirements.to_vec(),
            InstallAction::Update,
            printer,
        )?;
    }

    Ok(ExitStatus::Success)
}
