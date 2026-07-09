#![doc = "Routing primitives shared by server and local client."]

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterPlan {
    name: String,
}

impl RouterPlan {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::RouterPlan;

    #[test]
    fn creates_bootstrap_plan() {
        assert_eq!(RouterPlan::new("bootstrap").name(), "bootstrap");
    }
}
