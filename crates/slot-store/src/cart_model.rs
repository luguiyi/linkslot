use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CartModel {
    #[default]
    Famicom,
    Nes,
}

impl CartModel {
    pub fn parse(value: &str) -> Option<CartModel> {
        match value.trim().to_ascii_lowercase().as_str() {
            "famicom" | "fc" | "default" | "" => Some(CartModel::Famicom),
            "nes" => Some(CartModel::Nes),
            _ => None,
        }
    }

    pub fn to_value(self) -> &'static str {
        match self {
            CartModel::Famicom => "famicom",
            CartModel::Nes => "nes",
        }
    }

    pub fn from_meta(extra: &std::collections::HashMap<String, Value>) -> CartModel {
        extra
            .get("cart_model")
            .and_then(Value::as_str)
            .and_then(CartModel::parse)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::CartModel;

    #[test]
    fn parses_both_shell_names_and_the_default() {
        assert_eq!(CartModel::parse("famicom"), Some(CartModel::Famicom));
        assert_eq!(CartModel::parse("FC"), Some(CartModel::Famicom));
        assert_eq!(CartModel::parse("nes"), Some(CartModel::Nes));
        assert_eq!(CartModel::parse("SNES"), None);
        assert_eq!(CartModel::default(), CartModel::Famicom);
    }
}
