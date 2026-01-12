use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub(crate) fn spawn_kitty_with_current_exe() -> anyhow::Result<Child> {
    let exe = std::env::current_exe()?;
    spawn_kitty(exe, None)
}

pub(crate) fn spawn_kitty_reader_with_current_exe(
    book_path: &str,
    page_index: u32,
) -> anyhow::Result<Child> {
    let exe = std::env::current_exe()?;
    spawn_kitty(
        exe,
        Some(ReaderBootstrap {
            book_path,
            page_index,
        }),
    )
}

struct ReaderBootstrap<'a> {
    book_path: &'a str,
    page_index: u32,
}

fn spawn_kitty(exe: PathBuf, reader: Option<ReaderBootstrap<'_>>) -> anyhow::Result<Child> {
    let kitty =
        find_kitty_executable().ok_or_else(|| anyhow::anyhow!("`kitty` not found on PATH"))?;

    let mut cmd = Command::new(kitty);
    cmd.arg("--title").arg("bookshelf").arg("--").arg(exe);

    // If `kitty` is launched from within tmux or other nested environments, various env vars can
    // leak and confuse `ratatui-image` into emitting tmux passthrough wrappers. That results in raw
    // `_G...` sequences/base64 being printed instead of images.
    cmd.env_remove("TMUX");
    cmd.env_remove("TERM_PROGRAM");
    cmd.env_remove("TERM");

    if let Some(reader) = reader {
        cmd.env("BOOKSHELF_BOOT_READER", "1");
        cmd.env("BOOKSHELF_BOOT_READER_PATH", reader.book_path);
        cmd.env(
            "BOOKSHELF_BOOT_READER_PAGE_INDEX",
            reader.page_index.to_string(),
        );
        cmd.env("BOOKSHELF_BOOT_READER_MODE", "image");
    }

    // Avoid having child inherit raw-mode stdin.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    cmd.spawn().map_err(Into::into)
}

fn find_kitty_executable() -> Option<PathBuf> {
    find_on_path("kitty").or_else(|| find_on_path("kitty.exe"))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(name);
        if is_probably_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_probably_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(meta) = std::fs::metadata(path) {
            return meta.permissions().mode() & 0o111 != 0;
        }
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_kitty_on_path() {
        let base =
            std::env::temp_dir().join(format!("bookshelf-kitty-spawn-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let bin = base.join("kitty");
        fs::write(&bin, b"#!/bin/sh\nexit 0\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut perms = fs::metadata(&bin).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&bin, perms).unwrap();
        }

        let prev = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", &base);
        }
        let found = find_on_path("kitty");
        if let Some(prev) = prev {
            unsafe {
                std::env::set_var("PATH", prev);
            }
        } else {
            unsafe {
                std::env::remove_var("PATH");
            }
        }

        assert_eq!(found.as_deref(), Some(bin.as_path()));
        let _ = fs::remove_dir_all(&base);
    }
}
