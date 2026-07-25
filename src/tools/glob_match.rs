pub fn glob_match(pattern: &str, path: &str) -> bool {
    let mut pi = 0;
    let mut si = 0;
    let mut star_idx = None;
    let mut match_idx = 0;
    let pattern_bytes = pattern.as_bytes();
    let path_bytes = path.as_bytes();

    while si < path_bytes.len() {
        if pi < pattern_bytes.len() && pattern_bytes[pi] == b'*' {
            star_idx = Some(pi);
            match_idx = si;
            pi += 1;
        } else if pi < pattern_bytes.len()
            && (pattern_bytes[pi] == b'?' || pattern_bytes[pi] == path_bytes[si])
        {
            pi += 1;
            si += 1;
        } else if let Some(si_val) = star_idx {
            pi = si_val + 1;
            match_idx += 1;
            si = match_idx;
        } else {
            return false;
        }
    }

    while pi < pattern_bytes.len() && pattern_bytes[pi] == b'*' {
        pi += 1;
    }

    pi == pattern_bytes.len()
}
