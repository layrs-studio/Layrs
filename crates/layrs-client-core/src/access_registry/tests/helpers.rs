    fn numbered_lines(prefix: &str, count: usize) -> String {
        let mut text = String::new();
        for index in 0..count {
            text.push_str(prefix);
            text.push('-');
            text.push_str(&index.to_string());
            text.push('\n');
        }
        text
    }

    fn directory_file_size(path: &Path, extension: Option<&str>) -> u64 {
        if !path.exists() {
            return 0;
        }
        fs::read_dir(path)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && extension.is_none_or(|extension| {
                        path.extension().and_then(|value| value.to_str()) == Some(extension)
                    })
            })
            .filter_map(|path| fs::metadata(path).ok().map(|metadata| metadata.len()))
            .sum()
    }

    fn directory_file_count(path: &Path, extension: Option<&str>) -> usize {
        if !path.exists() {
            return 0;
        }
        fs::read_dir(path)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && extension.is_none_or(|extension| {
                        path.extension().and_then(|value| value.to_str()) == Some(extension)
                    })
            })
            .count()
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("layrs-{name}-{}", unix_now()));
        let _ = fs::remove_dir_all(&path);
        path
    }
