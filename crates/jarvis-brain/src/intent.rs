use serde::{Deserialize, Serialize};

/// Action comprise à partir d'une phrase : un outil et ses arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub tool: String,
    pub args: serde_json::Value,
}

impl Intent {
    pub fn new(tool: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            tool: tool.into(),
            args,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_construit_l_intention() {
        let i = Intent::new("open_app", serde_json::json!({"name": "code"}));
        assert_eq!(i.tool, "open_app");
        assert_eq!(i.args["name"], "code");
    }
}
