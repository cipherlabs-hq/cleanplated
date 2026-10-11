use std::env;

pub fn var(key: &str) -> Result<String, env::VarError> {
    if let Some(suffix) = key.strip_prefix("TRUSTARANT_") {
        let cleanplated_key = format!("CLEANPLATED_{suffix}");
        return env::var(&cleanplated_key).or_else(|_| env::var(key));
    }

    if let Some(suffix) = key.strip_prefix("CLEANPLATED_") {
        let trustarant_key = format!("TRUSTARANT_{suffix}");
        return env::var(key).or_else(|_| env::var(&trustarant_key));
    }

    env::var(key)
}
