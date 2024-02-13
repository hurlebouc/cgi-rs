use std::path::PathBuf;

struct Script {
    // Path to the CGI executable
    path: PathBuf,

    // URI, empty for "/"
    root: PathBuf,

    // Working directory of the CGI executable.
    // If None, base directory of path is used.
    // If path as no base directory, dir is used
    dir: Option<PathBuf>,

    // Environment variables
    env: Vec<(String, String)>,

    // Arguments of the CGI executable
    args: Vec<String>,
}

impl Script {
    fn server(&self) {}
}
