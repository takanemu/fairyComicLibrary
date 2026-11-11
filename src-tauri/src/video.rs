use std::{fs::File, io::BufReader, path::Path};

pub fn duration_seconds(path: &Path, format: &str) -> Result<u64, String> {
    let file =
        File::open(path).map_err(|error| format!("動画ファイルを開けませんでした: {error}"))?;

    match format {
        "mkv" => {
            let video = matroska_demuxer::MatroskaFile::open(BufReader::new(file))
                .map_err(|error| format!("MKVの情報を読み取れませんでした: {error}"))?;
            if !video
                .tracks()
                .iter()
                .any(|track| track.track_type() == matroska_demuxer::TrackType::Video)
            {
                return Err("動画トラックが見つかりません。".to_string());
            }
            let info = video.info();
            let duration = info
                .duration()
                .ok_or_else(|| "動画に再生時間の情報がありません。".to_string())?;
            let seconds = duration * info.timestamp_scale().get() as f64 / 1_000_000_000.0;
            if !seconds.is_finite() || seconds < 0.0 || seconds >= u64::MAX as f64 {
                return Err("動画の再生時間が不正です。".to_string());
            }
            Ok(seconds.floor() as u64)
        }
        "mp4" => {
            let size = file
                .metadata()
                .map_err(|error| format!("動画ファイルの情報を取得できませんでした: {error}"))?
                .len();
            let video = mp4::Mp4Reader::read_header(BufReader::new(file), size)
                .map_err(|error| format!("MP4の情報を読み取れませんでした: {error}"))?;
            if !video
                .tracks()
                .values()
                .any(|track| matches!(track.track_type(), Ok(mp4::TrackType::Video)))
            {
                return Err("動画トラックが見つかりません。".to_string());
            }
            let header = &video.moov.mvhd;
            if header.timescale == 0
                || (video.is_fragmented() && header.duration == 0)
                || header.duration == u64::MAX
                || (header.version == 0 && header.duration == u64::from(u32::MAX))
            {
                return Err("動画に有効な再生時間の情報がありません。".to_string());
            }
            Ok(header.duration / u64::from(header.timescale))
        }
        _ => Err("再生時間を取得できるのはMKVまたはMP4動画のみです。".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::duration_seconds;
    use base64::{engine::general_purpose::STANDARD, Engine};

    // Two 2.5-second, 16x16 test videos generated with FFmpeg.
    const MP4: &str = "AAAAHGZ0eXBpc29tAAACAGlzb21pc28ybXA0MQAAAAhmcmVlAAAANW1kYXQAAAGzABAHAAABthYFGCPbfgAAAbZfAjcAAAG2awEbAAABtl8CNwAAAbZrARsAAANobW9vdgAAAGxtdmhkAAAAAAAAAAAAAAAAAAAD6AAACcQAAQAAAQAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAApN0cmFrAAAAXHRraGQAAAADAAAAAAAAAAAAAAABAAAAAAAACcQAAAAAAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAABAAAAAQAAAAAAAkZWR0cwAAABxlbHN0AAAAAAAAAAEAAAnEAAAAAAABAAAAAAILbWRpYQAAACBtZGhkAAAAAAAAAAAAAAAAAABAAAAAoABVxAAAAAAALWhkbHIAAAAAAAAAAHZpZGUAAAAAAAAAAAAAAABWaWRlb0hhbmRsZXIAAAABtm1pbmYAAAAUdm1oZAAAAAEAAAAAAAAAAAAAACRkaW5mAAAAHGRyZWYAAAAAAAAAAQAAAAx1cmwgAAAAAQAAAXZzdGJsAAAA6nN0c2QAAAAAAAAAAQAAANptcDR2AAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAABAAEABIAAAASAAAAAAAAAABE0xhdmM2Mi4xMS4xMDAgbXBlZzQAAAAAAAAAAAAAAAAAGP//AAAAYGVzZHMAAAAAA4CAgE8AAQAEgICAQSARAAAAAAMNQAAAAJAFgICALwAAAbABAAABtYkTAAABAAAAASAAxI2IABUAhAIUYwAAAbJMYXZjNjIuMTEuMTAwBoCAgAECAAAAEHBhc3AAAAABAAAAAQAAABRidHJ0AAAAAAADDUAAAACQAAAAGHN0dHMAAAAAAAAAAQAAAAUAACAAAAAAFHN0c3MAAAAAAAAAAQAAAAEAAAAcc3RzYwAAAAAAAAABAAAAAQAAAAUAAAABAAAAKHN0c3oAAAAAAAAAAAAAAAUAAAARAAAABwAAAAcAAAAHAAAABwAAABRzdGNvAAAAAAAAAAEAAAAsAAAAYXVkdGEAAABZbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAbWRpcmFwcGwAAAAAAAAAAAAAAAAsaWxzdAAAACSpdG9vAAAAHGRhdGEAAAABAAAAAExhdmY2Mi4zLjEwMA==";
    const MKV: &str = "GkXfo6NChoEBQveBAULygQRC84EIQoKIbWF0cm9za2FCh4EEQoWBAhhTgGcBAAAAAAACiRFNm3TAv4Qs8bQNTbuLU6uEFUmpZlOsgaFNu4tTq4QWVK5rU6yB7027jFOrhBJUw2dTrIIBjk27jFOrhBxTu2tTrIICbewBAAAAAAAAUwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAFUmpZsm/hKl46Qkq17GDD0JATYCMTGF2ZjYyLjMuMTAwV0GMTGF2ZjYyLjMuMTAwc6SQhLJE10SwUB0RDujttqTUPkSJiECjiAAAAAAAFlSua0CZv4TjHMiArgEAAAAAAACK14EBc8WIA8RRxAwxhNWcgQAitZyDdW5kiIEAho9WX01QRUc0L0lTTy9BU1CDgQEj44OEHc1lAOCQsIEQuoEQmoECVbCEVbmBAVXugQDsAQAAAAAAAAIAAGOirwAAAbABAAABtYkTAAABAAAAASAAxI2IABUAhAIUYwAAAbJMYXZjNjIuMTEuMTAwElTDZ0CAv4TSNi2Wc3OfY8CAZ8iZRaOHRU5DT0RFUkSHjExhdmY2Mi4zLjEwMHNz1WPAi2PFiAPEUcQMMYTVZ8igRaOHRU5DT0RFUkSHk0xhdmM2Mi4xMS4xMDAgbXBlZzRnyKFFo4hEVVJBVElPTkSHkzAwOjAwOjAyLjUwMDAwMDAwMAAfQ7Z11L+ERtTqr+eBAKOVgQAAgAAAAbMAEAcAAAG2FgUYI9t+o4uBAfQAAAABtl8CN6OLgQPoAAAAAbZrARuji4EF3AAAAAG2XwI3o4uBB9AAAAABtmsBGxxTu2uXv4TrEUugu4+zgQC3iveBAfGCAhTwgQk=";

    #[test]
    fn reads_duration_from_mp4_and_mkv_without_decoding_video() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for (format, fixture) in [("mp4", MP4), ("mkv", MKV)] {
            let path = directory.path().join(format!("test.{format}"));
            std::fs::write(&path, STANDARD.decode(fixture).expect("decode fixture"))
                .expect("write fixture");
            assert_eq!(duration_seconds(&path, format).expect("duration"), 2);
        }
    }

    #[test]
    fn respects_matroska_timestamp_scale_and_mp4_timescale() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut mkv = STANDARD.decode(MKV).expect("decode MKV fixture");
        let scale = [0x2a, 0xd7, 0xb1, 0x83, 0x0f, 0x42, 0x40];
        let offset = mkv
            .windows(scale.len())
            .position(|part| part == scale)
            .unwrap();
        mkv[offset + 4..offset + 7].copy_from_slice(&[0x1e, 0x84, 0x80]);
        let mkv_path = directory.path().join("scaled.mkv");
        std::fs::write(&mkv_path, mkv).unwrap();
        assert_eq!(duration_seconds(&mkv_path, "mkv").unwrap(), 5);

        let mut mp4 = STANDARD.decode(MP4).expect("decode MP4 fixture");
        let offset = mp4.windows(4).position(|part| part == b"mvhd").unwrap();
        mp4[offset + 20..offset + 24].copy_from_slice(&3_661_000_u32.to_be_bytes());
        let mp4_path = directory.path().join("long.mp4");
        std::fs::write(&mp4_path, &mp4).unwrap();
        assert_eq!(duration_seconds(&mp4_path, "mp4").unwrap(), 3_661);

        mp4[offset + 16..offset + 20].fill(0);
        std::fs::write(&mp4_path, mp4).unwrap();
        assert!(duration_seconds(&mp4_path, "mp4").is_err());
    }

    #[test]
    fn reports_invalid_files_and_unsupported_formats() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("broken.mp4");
        std::fs::write(&path, b"broken").expect("write invalid fixture");
        assert!(duration_seconds(&path, "mp4").is_err());
        assert!(duration_seconds(&path, "mkv").is_err());
        assert!(duration_seconds(&path, "zip").is_err());
        assert!(duration_seconds(&directory.path().join("missing.mp4"), "mp4").is_err());
    }

    #[test]
    fn reads_duration_of_video_already_registered_in_repository() {
        let directory = tempfile::tempdir().expect("temporary repository");
        crate::repository::open(directory.path()).expect("initialize repository");
        let id = "ab".repeat(32);
        let relative = "archives/ab/video.mp4";
        std::fs::create_dir_all(directory.path().join("archives/ab")).unwrap();
        std::fs::write(
            directory.path().join(relative),
            STANDARD.decode(MP4).unwrap(),
        )
        .unwrap();
        let connection =
            rusqlite::Connection::open(directory.path().join("database/library.sqlite3")).unwrap();
        connection
            .execute(
                "INSERT INTO archives (archive_id, original_filename, relative_path, format,
                    file_size, imported_at)
                 VALUES (?1, 'video.mp4', ?2, 'mp4', 1, '2026-01-01T00:00:00Z')",
                rusqlite::params![id, relative],
            )
            .unwrap();
        let (path, format) = crate::maintenance::archive_for_open(directory.path(), &id).unwrap();
        assert_eq!(duration_seconds(&path, &format).unwrap(), 2);
    }
}
