pub const PRODUCT_NAME: &str = "AgentTerm";

pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("AgentTerm should start");
}

#[cfg(test)]
mod tests {
    use super::PRODUCT_NAME;

    #[test]
    fn product_name_is_stable() {
        assert_eq!(PRODUCT_NAME, "AgentTerm");
    }
}
