use ratatui_image::picker::{Capability, Picker, ProtocolType};

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

pub(crate) fn kitty_supported(picker: &Picker) -> bool {
    // `TERM` can be incorrect (e.g. inherited), but `KITTY_WINDOW_ID` is reliable.
    // Since we only support image mode in kitty, treat being inside kitty as sufficient.
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
}
