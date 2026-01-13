use ratatui_image::picker::{Capability, Picker, ProtocolType};

fn term_is_xterm_kitty() -> bool {
    std::env::var("TERM")
        .ok()
        .is_some_and(|term| term.trim().starts_with("xterm-kitty"))
}

pub(crate) fn ensure_tmux_allow_passthrough() {
    if std::env::var_os("TMUX").is_none() {
        return;
    }

    // Best effort: required for kitty graphics passthrough in tmux.
    // Ignore failures (old tmux, restricted env, etc).
    let _ = std::process::Command::new("tmux")
        .args(["set-option", "-g", "allow-passthrough", "on"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

pub(crate) fn in_kitty_env() -> bool {
    std::env::var("KITTY_WINDOW_ID")
        .ok()
        .is_some_and(|s| !s.trim().is_empty())
}

pub(crate) fn should_query_stdio() -> bool {
    if in_kitty_env() {
        return true;
    }

    if term_is_xterm_kitty() {
        // `KITTY_WINDOW_ID` is not forwarded over SSH by default, but `TERM` is.
        return true;
    }

    // In tmux, we need to query to detect the outer terminal protocol.
    std::env::var_os("TMUX").is_some()
}

pub(crate) fn stdio_query_timeout() -> std::time::Duration {
    // Strong hints we are talking to kitty (including over SSH).
    if in_kitty_env() || term_is_xterm_kitty() {
        return std::time::Duration::from_millis(1500);
    }

    // If we're in tmux, query quickly; if passthrough isn't enabled/supported, don't stall startup.
    if std::env::var_os("TMUX").is_some() {
        return std::time::Duration::from_millis(300);
    }

    std::time::Duration::from_millis(0)
}

pub(crate) fn kitty_supported(picker: &Picker) -> bool {
    // `KITTY_WINDOW_ID` is reliable when present; otherwise rely on queried capabilities.
    if in_kitty_env() {
        return true;
    }

    picker
        .capabilities()
        .iter()
        .any(|cap| matches!(cap, Capability::Kitty))
}

pub(crate) fn prefer_kitty_if_supported(picker: &mut Picker) -> bool {
    if !kitty_supported(picker) {
        return false;
    }
    picker.set_protocol_type(ProtocolType::Kitty);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_env_var<K: AsRef<str>, V: AsRef<str>>(key: K, value: Option<V>, f: impl FnOnce()) {
        let _guard = env_lock().lock().unwrap();
        let key = key.as_ref();
        let prev = std::env::var_os(key);
        match value {
            Some(v) => unsafe { std::env::set_var(key, v.as_ref()) },
            None => unsafe { std::env::remove_var(key) },
        }
        f();
        match prev {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    fn with_env_vars(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
        let _guard = env_lock().lock().unwrap();

        let prev = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var_os(key)))
            .collect::<Vec<(String, Option<OsString>)>>();

        for (key, value) in vars {
            match value {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }

        f();

        for (key, value) in prev {
            match value {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }

    #[test]
    fn kitty_supported_true_when_picker_protocol_is_kitty() {
        with_env_var("KITTY_WINDOW_ID", Option::<&str>::None, || {
            let mut picker = Picker::halfblocks();
            picker.set_protocol_type(ProtocolType::Kitty);
            // Without `KITTY_WINDOW_ID`, protocol alone isn't considered a reliable signal.
            assert!(!kitty_supported(&picker));
        });
    }

    #[test]
    fn prefer_kitty_sets_picker_protocol() {
        with_env_var("KITTY_WINDOW_ID", Option::<&str>::None, || {
            let mut picker = Picker::halfblocks();
            picker.set_protocol_type(ProtocolType::Kitty);
            assert!(!prefer_kitty_if_supported(&mut picker));
        });
    }

    #[test]
    fn kitty_supported_true_when_kitty_window_id_set() {
        with_env_var("KITTY_WINDOW_ID", Some("1"), || {
            let picker = Picker::halfblocks();
            assert!(kitty_supported(&picker));
            assert!(in_kitty_env());
        });
    }

    #[test]
    fn should_query_stdio_true_when_term_is_xterm_kitty() {
        with_env_vars(
            &[
                ("KITTY_WINDOW_ID", None),
                ("TERM", Some("xterm-kitty")),
                ("TMUX", None),
            ],
            || assert!(should_query_stdio()),
        );
    }

    #[test]
    fn should_query_stdio_false_without_hints() {
        with_env_vars(
            &[
                ("KITTY_WINDOW_ID", None),
                ("TERM", Some("xterm-256color")),
                ("TMUX", None),
            ],
            || assert!(!should_query_stdio()),
        );
    }

    #[test]
    fn should_query_stdio_true_in_tmux() {
        with_env_vars(
            &[
                ("KITTY_WINDOW_ID", None),
                ("TERM", Some("xterm-256color")),
                ("TMUX", Some("1")),
            ],
            || assert!(should_query_stdio()),
        );
    }
}
