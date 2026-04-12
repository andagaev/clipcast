mod clip;
mod duration;
mod paths;
mod pipeline;
mod preflight;
mod process;
mod sidecar;

fn main() {
    let _ = duration::parse;
    let _ = paths::default_output;
    let _ = paths::sidecar_for;
    let _ = sidecar::read;
    let _ = sidecar::write;
    let _ = sidecar::build;
    let _ = |s: sidecar::Sidecar| {
        let sidecar::Sidecar {
            clipcast_version,
            generated_at,
            target_duration_s,
            clips,
        } = s;
        (clipcast_version, generated_at, target_duration_s, clips)
    };
    let _ = |o: process::Output| {
        let process::Output { stdout, stderr } = o;
        (stdout, stderr)
    };
    let _ = process::ProcessError::KilledBySignal {
        program: String::new(),
    };
    let _ = process::run::<[&str; 0], &str, [(&str, &str); 0], &str, &str>;
    let _ = preflight::check_binaries;
    let _ = preflight::check_input_dir;
    let _ = preflight::REQUIRED_BINARIES;
    let _ = pipeline::discover::run;
    let _ = pipeline::frames::run;
    let _ = pipeline::frames::FRAMES_PER_CLIP;
    let _ = pipeline::frames::FRAME_MAX_WIDTH;
    let _ = |f: pipeline::frames::ClipFrames| {
        let pipeline::frames::ClipFrames { clip, frame_paths } = f;
        (clip, frame_paths)
    };
    let _ = |c: clip::Clip| {
        let clip::Clip { path, meta } = c;
        let clip::ClipMeta {
            duration_s,
            width,
            height,
            timestamp,
            timestamp_source,
        } = meta;
        (path, duration_s, width, height, timestamp, timestamp_source)
    };
    let _ = |v: clip::ClipVerdict| {
        let clip::ClipVerdict {
            path,
            duration_s,
            timestamp,
            timestamp_source,
            score,
            reason,
            error,
            keep,
        } = v;
        (
            path,
            duration_s,
            timestamp,
            timestamp_source,
            score,
            reason,
            error,
            keep,
        )
    };
    let _ = clip::TimestampSource::CreationTime;
    let _ = clip::TimestampSource::FilenamePattern;
    let _ = clip::TimestampSource::FileMtime;
    println!("clipcast v{}", env!("CARGO_PKG_VERSION"));
}
