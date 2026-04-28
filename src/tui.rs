//! Thin wrappers around cliclack for interactive terminal prompts.
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use crate::error::{AppError, Result};

pub fn can_run() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

fn map_err(e: io::Error) -> AppError {
    if e.kind() == io::ErrorKind::Interrupted {
        AppError::Cancelled
    } else {
        AppError::Read {
            path: PathBuf::from("<terminal>"),
            source: e,
        }
    }
}

const MAX_VISIBLE_ROWS: usize = 20;

pub fn select_one(title: &str, items: &[(String, String, String)]) -> Result<String> {
    let mut sel = cliclack::select(title);
    for (value, label, hint) in items {
        sel = sel.item(value.clone(), label, hint);
    }
    sel.max_rows(MAX_VISIBLE_ROWS).interact().map_err(map_err)
}

pub fn select_many(
    title: &str,
    items: &[(String, String, String)],
    initial: &[String],
) -> Result<Vec<String>> {
    let mut ms = cliclack::multiselect(title);
    for (value, label, hint) in items {
        ms = ms.item(value.clone(), label, hint);
    }
    if !initial.is_empty() {
        ms = ms.initial_values(initial.to_vec());
    }
    ms.required(false)
        .max_rows(MAX_VISIBLE_ROWS)
        .interact()
        .map_err(map_err)
}

pub fn confirm(title: &str) -> Result<bool> {
    cliclack::confirm(title)
        .initial_value(true)
        .interact()
        .map_err(map_err)
}

pub fn input(title: &str, placeholder: &str) -> Result<String> {
    cliclack::input(title)
        .placeholder(placeholder)
        .required(false)
        .interact::<String>()
        .map_err(map_err)
}
