//! Fixtures shared by tests in several workspaces (feature `test-fixtures`).

/// File paths whose characters URL, query, filesystem or Unicode handling could
/// change on the way to the content store: reserved URL characters, names that
/// read as a query, case variants, NFC and NFD forms, and names built from
/// those pieces with a fixed seed. Every path is a valid relative content path
/// without `.` or `..` segments, and no two are equal.
pub fn literal_content_paths() -> Vec<String> {
    let mut paths: Vec<String> = [
        "a&offset=1",
        "file&offset=1&length=2",
        "q?x=1",
        "hash#frag",
        "100%",
        "pct%41",
        "a=b",
        "plus+sign",
        "semi;colon",
        "space name",
        " lead and trail ",
        "it's",
        "quote\"d",
        "caf\u{e9}",
        "cafe\u{301}",
        "Case.txt",
        "case.txt",
        "CASE.TXT",
        "\u{65e5}\u{672c}.md",
        "dir/sub file&x=1",
        "dir/Sub File",
        "tab\tname",
    ]
    .iter()
    .map(|path| path.to_string())
    .collect();
    let pieces = [
        "%", "&", "?", "#", "=", "+", ";", " ", "'", "\"", "a", "B", "\u{e9}", "e\u{301}", "%2F",
        "%20", "x",
    ];
    let mut state: u64 = 0x5eed_c1d5_0000_0001;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    while paths.len() < 64 {
        let segments: Vec<String> = (0..1 + next() % 3)
            .map(|_| {
                (0..1 + next() % 6)
                    .map(|_| pieces[(next() % pieces.len() as u64) as usize])
                    .collect()
            })
            .collect();
        let path = segments.join("/");
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}
