use std::env;
use std::fs::File;
use std::io::{self, BufRead, Write};
use std::process;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

extern crate reqwest;

// Struct to hold parsed configuration and URLs
struct Config {
    urls: Vec<String>,
    workers: usize,
    timeout: Duration,
    retries: usize,
}

// Define the WebsiteStatus structure as per requirements
// #[derive(Debug)]
struct WebsiteStatus {
    url: String,
    action_status: Result<u16, String>, // HTTP code or error text
    response_time: Duration,
    timestamp: SystemTime,
}

// Helper function to print the usage message
fn print_usage() {
    eprintln!("Usage: website_checker [--file sites.txt] [suspicious link removed]");
    eprintln!("               [--workers N] [--timeout S] [--retries N]");
    eprintln!("\nOptions:");
    eprintln!("  --file <path>    Read URLs from a text file, one per line.");
    eprintln!("  [suspicious link removed]        Specify URLs directly as arguments.");
    eprintln!(
        "  --workers N      Number of concurrent worker threads (default: logical CPU cores)."
    );
    eprintln!("  --timeout S      Per-request timeout in seconds (default: 5).");
    eprintln!("  --retries N      Additional retry attempts after failure (default: 0).");
}

fn main() -> Result<(), String> {
    // --- Argument parsing and config creation ---
    let args: Vec<String> = env::args().collect();
    let mut iter = args.into_iter().skip(1);

    let mut file_path: Option<String> = None;
    let mut urls: Vec<String> = Vec::new();
    let mut workers: Option<usize> = None;
    let mut timeout_sec: Option<u64> = None;
    let mut retries: Option<usize> = None;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--file" => {
                file_path = Some(
                    iter.next()
                        .ok_or("--file requires a path argument".to_string())?,
                )
            }
            "--workers" => {
                let workers_str = iter
                    .next()
                    .ok_or("--workers requires a number argument".to_string())?;
                workers = Some(
                    workers_str
                        .parse()
                        .map_err(|e| format!("--workers must be a number: {}", e))?,
                );
            }
            "--timeout" => {
                let timeout_str = iter
                    .next()
                    .ok_or("--timeout requires a number argument".to_string())?;
                timeout_sec = Some(
                    timeout_str
                        .parse()
                        .map_err(|e| format!("--timeout must be a number in seconds: {}", e))?,
                );
            }
            "--retries" => {
                let retries_str = iter
                    .next()
                    .ok_or("--retries requires a number argument".to_string())?;
                retries = Some(
                    retries_str
                        .parse()
                        .map_err(|e| format!("--retries must be a number: {}", e))?,
                );
            }
            _ if arg.starts_with("--") => return Err(format!("Unknown argument: {}", arg)),
            url => urls.push(url.to_string()),
        }
    }

    if let Some(path) = file_path {
        let file = File::open(&path).map_err(|e| format!("Could not open file {}: {}", path, e))?;
        let reader = io::BufReader::new(file);
        for line in reader.lines() {
            let line = line.map_err(|e| format!("Error reading file {}: {}", path, e))?;
            let trimmed_line = line.trim();
            if !trimmed_line.is_empty() && !trimmed_line.starts_with('#') {
                urls.push(trimmed_line.to_string());
            }
        }
    }

    if urls.is_empty() {
        print_usage();
        process::exit(2);
    }

    let final_workers = workers.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(1)
    });
    let final_timeout = Duration::from_secs(timeout_sec.unwrap_or(5));
    let final_retries = retries.unwrap_or(0);

    let config = Config {
        urls,
        workers: final_workers,
        timeout: final_timeout,
        retries: final_retries,
    };
    // --- End Argument parsing and config creation ---

    println!(
        "Checking {} URLs with {} workers...",
        config.urls.len(),
        config.workers
    );

    // --- Threading and Website Checking ---

    // Create channels for sending URLs to workers and receiving results back
    let (url_sender, url_receiver) = mpsc::channel::<String>();
    // The results channel will carry WebsiteStatus structs
    let (results_sender, results_receiver) = mpsc::channel::<WebsiteStatus>();

    // Wrap the URL receiver in Arc<Mutex> to share it safely among multiple worker threads
    let shared_url_receiver = Arc::new(Mutex::new(url_receiver));

    // Spawn worker threads
    let mut worker_handles = Vec::with_capacity(config.workers);

    // Clone necessary config values for the worker threads
    let worker_timeout = config.timeout;
    let worker_retries = config.retries;

    for i in 0..config.workers {
        let worker_shared_url_receiver = Arc::clone(&shared_url_receiver); // Clone the Arc for each worker
        let worker_results_sender = results_sender.clone(); // Clone results sender for each worker

        let handle = thread::spawn(move || {
            // println!("Worker {} started.", i); // Can be noisy with many workers
            // Worker thread loop: Receive URLs until the channel is closed and empty
            // Lock the mutex to get exclusive access to the receiver
            while let Ok(url) = worker_shared_url_receiver.lock().unwrap().recv() {
                let mut attempts = 0;
                let mut status: Option<WebsiteStatus> = None;

                while attempts <= worker_retries {
                    let start_time = Instant::now();
                    let timestamp = SystemTime::now(); // Timestamp for *this* attempt

                    let check_result: Result<u16, String> = reqwest::blocking::ClientBuilder::new()
                        .timeout(worker_timeout)
                        .build()
                        .and_then(|client| client.get(&url).send())
                        .map_err(|e| e.to_string()) // Convert reqwest error to String
                        .and_then(|response| {
                            let status = response.status();
                            if status.is_success() || status.is_redirection() {
                                Ok(status.as_u16())
                            } else {
                                // Treat non-success/non-redirection status codes as errors for action_status
                                Err(format!("HTTP status code: {}", status.as_u16()))
                            }
                        });

                    let response_time = start_time.elapsed();

                    status = Some(WebsiteStatus {
                        url: url.clone(),                    // Clone URL for the status struct
                        action_status: check_result.clone(), // Clone result for retries check
                        response_time,
                        timestamp,
                    });

                    // Check if retry is needed
                    if status.as_ref().unwrap().action_status.is_err() && attempts < worker_retries
                    {
                        eprintln!(
                            "Worker {} failed checking {}. Retrying (Attempt {}/{})...",
                            i,
                            url,
                            attempts + 1,
                            worker_retries
                        ); // Use eprintln for retry info
                        thread::sleep(Duration::from_millis(100)); // Pause before retry
                        attempts += 1;
                    } else {
                        // Either success or no more retries
                        break;
                    }
                }

                // Send the final status (after retries) back to the main thread
                if let Some(final_status) = status {
                    if let Err(e) = worker_results_sender.send(final_status) {
                        // Error sending result back (likely main thread finished or panicked)
                        eprintln!("Worker {} failed to send result for {}: {}", i, url, e);
                        // No need to break here, the receiver will eventually error out if main is gone.
                    }
                }
            }
            // The results_sender clone is dropped when the worker thread exits the closure
            // println!("Worker {} shutting down.", i); // Can be noisy
        });
        worker_handles.push(handle);
    }

    // Drop the original results_sender immediately after spawning workers.
    // The results_receiver will only disconnect once ALL senders (including clones in workers) are dropped.
    drop(results_sender);

    // Send URLs to the workers from the main thread
    let num_urls = config.urls.len();
    for url in config.urls {
        // If sending fails, it likely means all workers have panicked or somehow exited without consuming.
        if let Err(e) = url_sender.send(url) {
            eprintln!("Failed to send URL to worker queue: {}", e);
            // Break the loop if sending fails, as the workers are likely gone
            break;
        }
    }
    // Drop the original url_sender to signal to workers that no more URLs are coming.
    drop(url_sender);

    // Collect results from the results channel and print live output
    let mut results: Vec<WebsiteStatus> = Vec::with_capacity(num_urls);
    println!("\n--- Live Results ---"); // Header for live output
    // The results_receiver will receive until all results_senders (in workers) are dropped.
    for received_status in results_receiver {
        // Print live output immediately upon receiving a result
        let status_str = match &received_status.action_status {
            Ok(code) => format!("OK ({})", code),
            Err(err) => format!("ERROR ({})", err),
        };
        // Format duration nicely for live output (e.g., milliseconds)
        let response_time_ms = received_status.response_time.as_secs_f64() * 1000.0;
        println!(
            "{} | {} | {:.2} ms",
            received_status.url, status_str, response_time_ms
        );
        results.push(received_status);
    }
    println!("--- End Live Results ---");

    // Wait for all worker threads to finish
    // This is important to ensure all results are sent before the main thread potentially exits.
    // It also allows us to catch panics in workers.
    for (i, handle) in worker_handles.into_iter().enumerate() {
        // Joining allows us to catch panics in workers. Ignoring the Ok result is fine here.
        // If a worker panicked, join() will return an Err.
        if let Err(e) = handle.join() {
            eprintln!("Worker {} panicked: {:?}", i, e);
        }
        // println!("Worker {} joined.", i); // Can be noisy
    }

    println!("All checks complete. Collected {} results.", results.len());

    // --- JSON generation and writing ---
    let json_output_file = "status.json";
    println!("Generating JSON output: {}", json_output_file);

    let mut json_string = String::new();
    json_string.push_str("[\n"); // Start of JSON array

    for (i, status) in results.iter().enumerate() {
        if i > 0 {
            json_string.push_str(",\n"); // Add comma before each object except the first
        }
        json_string.push_str("  {\n"); // Start of JSON object

        // url field (string, must be escaped)
        json_string.push_str(&format!(
            "    \"url\": \"{}\",\n",
            escape_json_string(&status.url)
        ));

        // action_status field (object representing the Result)
        json_string.push_str("    \"action_status\": ");
        match &status.action_status {
            Ok(code) => {
                // Success case: object with status "success" and code
                json_string.push_str(&format!(
                    "{{\"status\": \"success\", \"code\": {}}},\n",
                    code
                ));
            }
            Err(err) => {
                // Error case: object with status "error" and message (must be escaped)
                json_string.push_str(&format!(
                    "{{\"status\": \"error\", \"message\": \"{}\"}},\n",
                    escape_json_string(err)
                ));
            }
        }

        // response_time field (in seconds as float)
        let response_time_sec = status.response_time.as_secs_f64();
        // Use {:.6} for reasonable precision as a float string in JSON
        json_string.push_str(&format!(
            "    \"response_time_sec\": {:.6},\n",
            response_time_sec
        ));

        // timestamp field (as Unix timestamp in seconds as float)
        // Handle potential error if timestamp is before epoch (e.g., return 0.0)
        let timestamp_sec = status
            .timestamp
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0); // Default to 0.0 or handle as an error if timestamp is before epoch

        json_string.push_str(&format!("    \"timestamp_unix\": {:.6}\n", timestamp_sec));

        json_string.push_str("  }"); // End of JSON object
    }

    json_string.push_str("\n]"); // End of JSON array

    // Write the final JSON string to the file
    let mut output_file = File::create(json_output_file)
        .map_err(|e| format!("Could not create output file {}: {}", json_output_file, e))?;

    output_file
        .write_all(json_string.as_bytes())
        .map_err(|e| format!("Could not write to output file {}: {}", json_output_file, e))?;

    println!("JSON output written successfully to {}", json_output_file);

    // --- End JSON generation and writing ---

    Ok(())
}

// Helper function for manual JSON string escaping
// Handles essential JSON escapes: ", \, /, \b, \f, \n, \r, \t and control characters < U+0020
fn escape_json_string(s: &str) -> String {
    let mut escaped = String::new();
    for c in s.chars() {
        match c {
            '"' => escaped.push_str("\\\""),       // Quotation mark
            '\\' => escaped.push_str("\\\\"),      // Reverse solidus
            '/' => escaped.push_str("\\/"), // Solidus (forward slash) - good practice to escape
            '\u{0008}' => escaped.push_str("\\b"), // Backspace
            '\u{000C}' => escaped.push_str("\\f"), // Form feed
            '\n' => escaped.push_str("\\n"), // Newline
            '\r' => escaped.push_str("\\r"), // Carriage return
            '\t' => escaped.push_str("\\t"), // Horizontal tab
            c if c.is_control() => {
                // Other control characters (U+0000 to U+001F)
                escaped.push_str(&format!("\\u{:04x}", c as u32));
            }
            _ => escaped.push(c), // Any other character is added as is
        }
    }
    escaped
}

// Documentation for JSON Field Names:
/*
The output file `status.json` is a JSON array of objects, where each object represents the status of a checked URL.
Each object has the following fields:
- `url`: String - The original URL that was checked. Special characters are JSON escaped.
- `action_status`: Object - Describes the outcome of the check.
    - If the check was successful (HTTP status 2xx or 3xx):
        `{"status": "success", "code": <HTTP status code as a number>}`
    - If the check failed (network error, timeout, or non-2xx/3xx status):
        `{"status": "error", "message": "<Error message string>"}` (Error message string is JSON escaped)
- `response_time_sec`: Number - The total time taken for the check (including retries and pauses), in seconds, as a floating-point number.
- `timestamp_unix`: Number - The timestamp when the *final* check attempt completed, as seconds since the Unix epoch (1970-01-01 00:00:00 UTC), as a floating-point number. A value of 0.0 may indicate an error getting the timestamp relative to the epoch.
*/
