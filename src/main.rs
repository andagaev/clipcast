mod clip;
mod duration;

fn main() {
    let _ = duration::parse;
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
