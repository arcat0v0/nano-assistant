use super::*;
use std::io::{Seek, SeekFrom};

const EXPANDED_LIMIT: u64 = 128 * 1024 * 1024;
const MEMBER_LIMIT: usize = 256;

struct BoundedReader<R> {
    inner: R,
    deadline: Instant,
    consumed: u64,
    limit: u64,
}
impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "archive deadline exceeded",
            ));
        }
        let allowed = self.limit.saturating_sub(self.consumed);
        if allowed == 0 {
            let mut byte = [0];
            let read = self.inner.read(&mut byte)?;
            if read == 0 {
                return Ok(0);
            }
            return Err(io::Error::other("archive expansion limit exceeded"));
        }
        let size = buffer.len().min(allowed.min(usize::MAX as u64) as usize);
        let count = self.inner.read(&mut buffer[..size])?;
        self.consumed += count as u64;
        Ok(count)
    }
}
struct Index {
    members: Vec<Value>,
    names: HashSet<String>,
    expanded: u64,
    deadline: Instant,
    truncated: bool,
    total_members: Option<usize>,
}
impl Index {
    fn member(
        &mut self,
        name: String,
        kind: &str,
        size: u64,
        reader: Option<&mut dyn Read>,
        link: Option<String>,
        context: &EvidenceContext,
    ) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "archive deadline exceeded",
            ));
        }
        if self.members.len() >= MEMBER_LIMIT {
            return Err(io::Error::other("archive member limit exceeded"));
        }
        if name.len() > 1024 {
            return Err(io::Error::other("archive member path limit exceeded"));
        }
        if name.starts_with('/')
            || name.starts_with('\\')
            || name.contains('\0')
            || name.split(['/', '\\']).any(|p| p == "..")
            || name.as_bytes().get(1) == Some(&b':')
        {
            return Err(io::Error::other("unsafe archive member path"));
        }
        let duplicate = !self.names.insert(name.clone());
        let sensitive = archive_sensitive(&name, context);
        let mut value = json!({"path":name,"type":kind,"size":size,"duplicate":duplicate,"sensitive":sensitive});
        if let Some(link) = link {
            value["link_target"] = json!(link);
        }
        if let Some(reader) = reader {
            let mut digest = Sha256::new();
            let mut bytes = 0u64;
            let mut buffer = [0u8; 32768];
            loop {
                if Instant::now() >= self.deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "archive deadline exceeded",
                    ));
                }
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                bytes += count as u64;
                self.expanded += count as u64;
                if self.expanded > EXPANDED_LIMIT {
                    return Err(io::Error::other("archive expanded bytes limit exceeded"));
                }
                if !sensitive {
                    digest.update(&buffer[..count]);
                }
            }
            if bytes != size {
                return Err(io::Error::other("archive member size mismatch"));
            }
            if kind == "file" && !sensitive {
                value["sha256"] = json!(format!("{:x}", digest.finalize()));
            }
        }
        self.members.push(value);
        if serde_json::to_vec(&self.members).map_or(true, |bytes| bytes.len() > 12 * 1024) {
            self.truncated = true;
            return Err(io::Error::other("archive index output limit exceeded"));
        }
        Ok(())
    }
}

pub(super) fn collect(request: &ArchiveRequest, context: &EvidenceContext) -> EvidenceResult {
    let tool = "review_archive";
    let path = match resolve(&request.path, context) {
        Ok(path) => path,
        Err(e) => return io_result(tool, &request.path, e),
    };
    let snapshot = match PathSnapshot::capture(&path) {
        Ok(snapshot) => snapshot,
        Err(e) => return io_result(tool, &request.path, e),
    };
    if snapshot.canonical.is_none() {
        let mut result = EvidenceResult::new(
            tool,
            EvidenceStatus::NotFound,
            &request.path,
            true,
            json!({"exists":false}),
        );
        result.snapshot = Some(snapshot);
        return result;
    }
    let canonical = snapshot.canonical.as_ref().unwrap();
    if forbidden_content(&path, context) || forbidden_content(canonical, context) {
        return EvidenceResult::error(
            tool,
            &request.path,
            EvidenceStatus::Redacted,
            "sensitive archive path withheld",
        );
    }
    let mut file = match open_snapshot(&snapshot) {
        Ok(file) => file,
        Err(e) => return io_result(tool, &request.path, e),
    };
    let metadata = match file.metadata() {
        Ok(m) => m,
        Err(e) => return io_result(tool, &request.path, e),
    };
    if metadata.len() > HASH_LIMIT {
        return EvidenceResult::error(
            tool,
            &request.path,
            EvidenceStatus::LimitExceeded,
            "compressed archive exceeds 64 MiB",
        );
    }
    let deadline = context
        .deadline
        .min(Instant::now() + Duration::from_secs(5));
    let mut index = Index {
        members: Vec::new(),
        names: HashSet::new(),
        expanded: 0,
        deadline,
        truncated: false,
        total_members: None,
    };
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let hash = (|| {
        let mut buffer = [0u8; 32768];
        loop {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "archive deadline exceeded",
                ));
            }
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
            if bytes > HASH_LIMIT {
                return Err(io::Error::other("compressed archive limit exceeded"));
            }
            digest.update(&buffer[..count]);
        }
        file.seek(SeekFrom::Start(0))?;
        Ok::<(), io::Error>(())
    })();
    if let Err(e) = hash {
        return io_result(tool, &request.path, e);
    }
    let archive_digest = format!("{:x}", digest.finalize());
    let mut magic = [0u8; 512];
    let count = match file.read(&mut magic) {
        Ok(n) => n,
        Err(e) => return io_result(tool, &request.path, e),
    };
    if let Err(e) = file.seek(SeekFrom::Start(0)) {
        return io_result(tool, &request.path, e);
    }
    let mut recognized = true;
    let outcome = if magic.starts_with(b"PK\x03\x04")
        || magic.starts_with(b"PK\x05\x06")
        || magic.starts_with(b"PK\x07\x08")
    {
        inspect_zip(&mut file, &mut index, context)
    } else if magic.starts_with(&[0x1f, 0x8b]) {
        let decoder = compressed_gzip(&mut file, deadline, HASH_LIMIT);
        inspect_tar(
            BoundedReader {
                inner: decoder,
                deadline,
                consumed: 0,
                limit: EXPANDED_LIMIT,
            },
            &mut index,
            context,
        )
    } else if count == 512 && (magic[257..262] == *b"ustar" || tar_header_valid(&magic)) {
        inspect_tar(
            BoundedReader {
                inner: &mut file,
                deadline,
                consumed: 0,
                limit: EXPANDED_LIMIT,
            },
            &mut index,
            context,
        )
    } else {
        recognized = false;
        Err(io::Error::other("unsupported archive format"))
    };
    if finish_snapshot(&snapshot, Some(&file)).is_err() {
        return EvidenceResult::error(
            tool,
            &request.path,
            EvidenceStatus::Changed,
            "archive changed during collection",
        );
    }
    let complete = outcome.is_ok() && !index.truncated;
    let mut data = json!({"metadata":metadata_json(&metadata),"sha256":archive_digest,"members":index.members,"expanded_member_bytes":index.expanded,"archive_read_complete":complete,"integrity_checked":complete,"truncated":!complete,"collected_members":index.members.len(),"total_members":index.total_members.or_else(||complete.then_some(index.members.len()))});
    let status = match outcome {
        Ok(()) => EvidenceStatus::Ok,
        Err(e) => {
            let message = e.to_string();
            data["error"] = json!(message.chars().take(1024).collect::<String>());
            if message.contains("limit") || e.kind() == io::ErrorKind::TimedOut {
                EvidenceStatus::LimitExceeded
            } else {
                EvidenceStatus::Unavailable
            }
        }
    };
    if !recognized {
        data["format_supported"] = json!(false);
    }
    let mut result = EvidenceResult::new(tool, status, &request.path, complete, data);
    result.snapshot = Some(snapshot);
    result
}
fn compressed_gzip<R: Read>(
    reader: R,
    deadline: Instant,
    limit: u64,
) -> flate2::read::MultiGzDecoder<BoundedReader<R>> {
    flate2::read::MultiGzDecoder::new(BoundedReader {
        inner: reader,
        deadline,
        consumed: 0,
        limit,
    })
}
fn archive_sensitive(name: &str, context: &EvidenceContext) -> bool {
    let components: Vec<_> = name
        .split(['/', '\\'])
        .filter(|component| !component.is_empty() && *component != ".")
        .collect();
    if components.iter().any(|component| {
        sensitive_name(component)
            || [".ssh", ".gnupg", ".aws", ".kube", ".docker"].contains(component)
    }) {
        return true;
    }
    if components.windows(2).any(|pair| {
        pair == ["etc", "shadow"] || pair == ["etc", "gshadow"] || pair == ["Develop", "op_secrets"]
    }) {
        return true;
    }
    forbidden_content(
        &context.home.as_ref().unwrap_or(&context.cwd).join(name),
        context,
    )
}

fn tar_header_valid(header: &[u8; 512]) -> bool {
    let checksum = std::str::from_utf8(&header[148..156])
        .ok()
        .and_then(|s| u64::from_str_radix(s.trim_matches(['\0', ' ']), 8).ok());
    let sum = header
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if (148..156).contains(&i) {
                32u64
            } else {
                u64::from(*b)
            }
        })
        .sum::<u64>();
    checksum == Some(sum)
}
fn inspect_zip(file: &mut File, index: &mut Index, context: &EvidenceContext) -> io::Result<()> {
    let reader = BoundedReader {
        inner: file,
        deadline: index.deadline,
        consumed: 0,
        limit: u64::MAX,
    };
    let mut archive = zip::ZipArchive::new(reader).map_err(io::Error::other)?;
    index.total_members = Some(archive.len());
    if archive.len() > MEMBER_LIMIT {
        return Err(io::Error::other("archive member limit exceeded"));
    }
    for number in 0..archive.len() {
        let mut member = archive.by_index(number).map_err(io::Error::other)?;
        let name = member.name().to_owned();
        let size = member.size();
        if size > EXPANDED_LIMIT.saturating_sub(index.expanded) {
            return Err(io::Error::other("archive expanded bytes limit exceeded"));
        }
        let mode = member.unix_mode().unwrap_or(0);
        let kind = if member.is_dir() {
            "directory"
        } else if mode & 0o170000 == 0o120000 {
            "symlink"
        } else if mode & 0o170000 != 0 && mode & 0o170000 != 0o100000 {
            "special"
        } else {
            "file"
        };
        index.member(name, kind, size, Some(&mut member), None, context)?;
    }
    Ok(())
}
impl<R: Seek> Seek for BoundedReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "archive deadline exceeded",
            ));
        }
        self.inner.seek(position)
    }
}
fn inspect_tar<R: Read>(reader: R, index: &mut Index, context: &EvidenceContext) -> io::Result<()> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut member = entry?;
        let name = member
            .path()?
            .to_str()
            .ok_or_else(|| io::Error::other("non UTF-8 archive member name"))?
            .to_owned();
        let size = member.size();
        let entry_type = member.header().entry_type();
        let kind = if entry_type.is_file() {
            "file"
        } else if entry_type.is_dir() {
            "directory"
        } else if entry_type.is_symlink() {
            "symlink"
        } else if entry_type.is_hard_link() {
            "hardlink"
        } else {
            "special"
        };
        let link = member
            .link_name()?
            .map(|p| p.to_string_lossy().into_owned());
        if size > EXPANDED_LIMIT.saturating_sub(index.expanded) {
            return Err(io::Error::other("archive expanded bytes limit exceeded"));
        }
        index.member(name, kind, size, Some(&mut member), link, context)?;
    }
    let mut reader = archive.into_inner();
    let mut buffer = [0u8; 32768];
    loop {
        if Instant::now() >= index.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "archive deadline exceeded",
            ));
        }
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if buffer[..count].iter().any(|byte| *byte != 0) {
            return Err(io::Error::other("nonzero trailing tar data"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[tokio::test]
    async fn security_evidence_archive_zip_digest_and_no_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("backup.zip")).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("data", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();
        let ctx = EvidenceContext::new(
            dir.path().into(),
            Some(dir.path().into()),
            None,
            Instant::now() + Duration::from_secs(5),
        );
        let result = ProductionEvidenceCollector::default()
            .collect(
                &EvidenceRequest::Archive(ArchiveRequest {
                    path: "backup.zip".into(),
                }),
                &ctx,
            )
            .await;
        assert_eq!(result.status, EvidenceStatus::Ok);
        assert_eq!(result.data["integrity_checked"], true);
        assert_eq!(
            result.data["members"][0]["sha256"],
            format!("{:x}", Sha256::digest(b"hello"))
        );
        assert!(!dir.path().join("data").exists());
    }
    #[tokio::test]
    async fn security_evidence_archive_tar_gzip_trailer_and_sensitive_members() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("backup.tgz")).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(6);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, ".env", b"secret".as_slice())
            .unwrap();
        let encoder = tar.into_inner().unwrap();
        encoder.finish().unwrap();
        let ctx = EvidenceContext::new(
            dir.path().into(),
            Some(dir.path().into()),
            None,
            Instant::now() + Duration::from_secs(5),
        );
        let request = EvidenceRequest::Archive(ArchiveRequest {
            path: "backup.tgz".into(),
        });
        let result = ProductionEvidenceCollector::default()
            .collect(&request, &ctx)
            .await;
        assert_eq!(result.data["integrity_checked"], true);
        assert!(result.data["members"][0].get("sha256").is_none());
        let mut bytes = std::fs::read(dir.path().join("backup.tgz")).unwrap();
        bytes.truncate(bytes.len() - 4);
        std::fs::write(dir.path().join("backup.tgz"), bytes).unwrap();
        let result = ProductionEvidenceCollector::default()
            .collect(&request, &ctx)
            .await;
        assert!(!result.complete);
        assert_ne!(result.data["integrity_checked"], true);
    }
    #[tokio::test]
    async fn security_evidence_archive_crc_and_unsafe_member_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = EvidenceContext::new(
            dir.path().into(),
            Some(dir.path().into()),
            None,
            Instant::now() + Duration::from_secs(5),
        );
        for (name, corrupt) in [("safe", true), ("../escape", false)] {
            let file = File::create(dir.path().join("archive.zip")).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(b"unique payload").unwrap();
            writer.finish().unwrap();
            if corrupt {
                let mut bytes = std::fs::read(dir.path().join("archive.zip")).unwrap();
                let index = bytes
                    .windows(14)
                    .position(|v| v == b"unique payload")
                    .unwrap();
                bytes[index] ^= 1;
                std::fs::write(dir.path().join("archive.zip"), bytes).unwrap();
            }
            let result = ProductionEvidenceCollector::default()
                .collect(
                    &EvidenceRequest::Archive(ArchiveRequest {
                        path: "archive.zip".into(),
                    }),
                    &ctx,
                )
                .await;
            assert!(!result.complete);
            assert_ne!(result.data["integrity_checked"], true);
            assert!(!dir.path().parent().unwrap().join("escape").exists());
        }
    }
    #[tokio::test]
    async fn security_evidence_archive_tar_duplicate_links_and_member_limit() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = EvidenceContext::new(
            dir.path().into(),
            Some(dir.path().into()),
            None,
            Instant::now() + Duration::from_secs(5),
        );
        let mut builder = tar::Builder::new(File::create(dir.path().join("archive.tar")).unwrap());
        for _ in 0..2 {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append_data(&mut header, "same", b"a".as_slice())
                .unwrap();
        }
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        builder.append_link(&mut header, "link", "same").unwrap();
        builder.finish().unwrap();
        drop(builder);
        let request = EvidenceRequest::Archive(ArchiveRequest {
            path: "archive.tar".into(),
        });
        let result = ProductionEvidenceCollector::default()
            .collect(&request, &ctx)
            .await;
        assert!(result.complete, "{result:?}");
        assert_eq!(result.data["members"][1]["duplicate"], true);
        assert_eq!(result.data["members"][2]["type"], "symlink");
        assert!(!dir.path().join("link").exists());
        let mut builder = tar::Builder::new(File::create(dir.path().join("archive.tar")).unwrap());
        for number in 0..257 {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("member{number}"), [].as_slice())
                .unwrap();
        }
        builder.finish().unwrap();
        drop(builder);
        let result = ProductionEvidenceCollector::default()
            .collect(&request, &ctx)
            .await;
        assert!(!result.complete);
        assert_eq!(result.status, EvidenceStatus::LimitExceeded);
        assert!(serde_json::to_vec(&result).unwrap().len() <= RESULT_LIMIT);
    }
    #[tokio::test]
    async fn security_evidence_archive_backup_credential_paths_have_no_digest() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = EvidenceContext::new(
            dir.path().into(),
            Some(dir.path().into()),
            None,
            Instant::now() + Duration::from_secs(5),
        );
        let mut builder = tar::Builder::new(File::create(dir.path().join("backup.tar")).unwrap());
        for name in [
            "home/alice/.kube/config",
            "backup/.docker/config.json",
            "etc/shadow",
            "home/alice/Develop/op_secrets/config",
            "ordinary",
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append_data(&mut header, name, b"data".as_slice())
                .unwrap();
        }
        builder.finish().unwrap();
        drop(builder);
        let result = ProductionEvidenceCollector::default()
            .collect(
                &EvidenceRequest::Archive(ArchiveRequest {
                    path: "backup.tar".into(),
                }),
                &ctx,
            )
            .await;
        assert!(result.complete);
        let members = result.data["members"].as_array().unwrap();
        for member in &members[..4] {
            assert_eq!(member["sensitive"], true);
            assert!(member.get("sha256").is_none());
        }
        assert!(members[4].get("sha256").is_some());
    }
    #[test]
    fn security_evidence_gzip_construction_obeys_compressed_deadline_and_limit() {
        let mut bytes = vec![0x1f, 0x8b, 8, 8, 0, 0, 0, 0, 0, 255];
        bytes.extend(std::iter::repeat_n(b'x', 1024));
        bytes.push(0);
        let mut decoder = compressed_gzip(
            bytes.as_slice(),
            Instant::now() - Duration::from_secs(1),
            HASH_LIMIT,
        );
        let mut output = Vec::new();
        assert_eq!(
            decoder.read_to_end(&mut output).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        let mut decoder = compressed_gzip(
            bytes.as_slice(),
            Instant::now() + Duration::from_secs(1),
            16,
        );
        assert!(decoder
            .read_to_end(&mut Vec::new())
            .unwrap_err()
            .to_string()
            .contains("limit"));
    }
}
