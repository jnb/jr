/// Assert a snapshot with a set of filters applied.
#[macro_export]
macro_rules! assert_snapshot_filtered {
    ($output:expr, $filters:expr, @$expected:literal) => {
        insta::with_settings!({filters => $filters.clone()}, {
            insta::assert_snapshot!($output, @$expected);
        });
    };
}

/// Run a command and capture its stdout and stderr.
#[macro_export]
macro_rules! run_and_capture {
    ($cmd:expr) => {{
        let mut out = Vec::new();
        let mut err = Vec::new();
        $cmd(&mut out, &mut err).await?;
        (String::from_utf8(out)?, String::from_utf8(err)?)
    }};
}
