fn main() {
    if let Err(error) = lmm::run() {
        if matches!(error, lmm::error::AppError::Cancelled) {
            return;
        }
        let msg = format!("{error}");
        if msg.contains("error sending request")
            || msg.contains("connection refused")
            || msg.contains("dns error")
        {
            eprintln!(
                "{} network error: could not reach Hugging Face. Check your internet connection.",
                lmm::format::red("error:")
            );
        } else {
            eprintln!("{} {error}", lmm::format::red("error:"));
        }
        std::process::exit(1);
    }
}
