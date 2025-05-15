Website Checker 🌐

A concurrent command-line utility written in Rust to check the availability of multiple websites in parallel.

Building 🏗️

To build the project, you need Rust and Cargo installed.

    Clone the repository:

    git clone https://github.com/jboca37/webcheckcli.git
    cd webcheckcli 

    Ensure you have the reqwest dependency specified in your Cargo.toml:

    [dependencies]
    reqwest = { version = "0.11", features = ["blocking"] } # Use an appropriate version

    Build the project:

    cargo build --release

    The executable will be located at target/release/webcheckcli. You can omit --release for a debug build (target/debug/webcheckcli), which is faster to compile but slower to run.

Usage 🚀

The program follows the following command-line syntax:

webcheckcli [--file sites.txt] [link]
               [--workers N] [--timeout S] [--retries N]

If neither --file nor positional URLs are provided, a usage message will be printed, and the program will exit with code 2.
Arguments:

    --file <path>: Read URLs from a text file. Each line is treated as a URL. Blank lines and lines starting with # are ignored.

    [link]: One or more URLs provided directly as command-line arguments.

    --workers N: The number of concurrent worker threads to use. Defaults to the number of logical CPU cores.

    --timeout S: The timeout for each individual HTTP request, in seconds. Defaults to 5 seconds.

    --retries N: The number of additional retry attempts to make after an initial request failure. Defaults to 0. There is a 100ms pause between retries.

Examples:

    Check a few URLs directly:

    ./target/release/webcheckcli https://www.google.com https://www.rust-lang.org

    Check URLs from a file named urls.txt:

    ./target/release/webcheckcli --file urls.txt

    Check URLs from a file and add another URL:

    ./target/release/webcheckcli --file urls.txt https://github.com

    Use 10 worker threads and a 15-second timeout:

    ./target/release/webcheckcli https://example.com --workers 10 --timeout 15

    Attempt up to 2 retries for each URL:

    ./target/release/webcheckcli http://unstable.site --retries 2
