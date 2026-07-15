#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/unit.rs");
    include!("tests/weaves.rs");
    include!("tests/store_objects.rs");

    mod weave_authz {
        use super::*;
        include!("tests/weave_authz.rs");
    }
}
