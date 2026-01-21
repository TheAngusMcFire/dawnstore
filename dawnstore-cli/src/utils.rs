use std::{
    io::Write,
    path::Path,
    process::Command,
    time::SystemTime,
};

use color_eyre::eyre::{self, Result};

fn get_last_modified_time(path: &Path) -> eyre::Result<SystemTime> {
    let metadata = std::fs::metadata(path)?; // Get file metadata
    let modified_time = metadata.modified()?; // Get the last modification time from metadata
    Ok(modified_time)
}

pub fn edit_with_default_editor<'r, S: Into<&'r str>>(
    msg: S,
) -> Result<Option<String>, color_eyre::Report> {
    let before_text = msg.into();
    let mut file = tempfile::Builder::new().suffix(".yaml").tempfile()?;
    file.write_all(before_text.as_bytes())?;
    let path = file.path();
    let before_edited_time = get_last_modified_time(path)?;

    let editor = std::env::var("EDITOR")?;
    let status = Command::new(editor).arg(path).spawn()?.wait()?;
    if !status.success() {
        eyre::bail!("editor did not indicate success code");
    }

    let after_edited_time = get_last_modified_time(path)?;

    let after_text = std::fs::read_to_string(file.path())?;

    if before_edited_time == after_edited_time {
        return Ok(None);
    }

    if before_text == after_text {
        return Ok(None);
    }

    Ok(Some(after_text))
}
