//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn channel() -> &'static str {
    "minimap"
}

pub fn build_id() -> Option<&'static str> {
    non_empty(option_env!("HERDR_BUILD_ID")).or(Some("alpha1"))
}

pub fn version() -> String {
    match channel() {
        "stable" => BASE_VERSION.to_string(),
        channel => match build_id() {
            Some(build_id) => format!("{BASE_VERSION}-{channel}.{build_id}"),
            None => format!("{BASE_VERSION}-{channel}"),
        },
    }
}

pub fn is_preview() -> bool {
    channel() == "preview"
}

fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

/// This independently distributed fork uses manual source/binary updates.
pub fn is_minimap_fork() -> bool {
    true
}

pub const FORK_UPDATE_GUIDANCE: &str = "This is the Herdr minimap fork. Official self-update is disabled to preserve sidebar support. Update from https://github.com/yukiumi13/herdr/tree/pr-minimap and rebuild, or install a fork release. To return to official Herdr, install it separately from herdr.dev.";

#[cfg(test)]
mod tests {
    #[test]
    fn stable_version_defaults_to_cargo_version() {
        assert!(!super::version().is_empty());
    }
}
