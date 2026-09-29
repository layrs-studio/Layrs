use super::*;
use serde_json::json;
fn capability() -> DownloadCapability {
    let id = format!("blake3:{}", "3".repeat(64));
    serde_json::from_value(json!({
        "principalId":"principal:alice", "authorizationEpoch":1,"spaceId":"space:demo",
        "clientId":"client:first","requestId":"request:first","planId":"plan:first",
        "claim":{"kind":"object","object":{"kind":"step","objectId":id},
          "root":{"kind":"step","stepId":id},"repoPath":null},
        "descriptor":{"kind":"step","objectId":id,"canonicalByteLen":100,"encodedByteLen":100,"compression":"identity"},
        "claimDigest":id,"method":"GET","downloadUrl":"https://example.invalid/v3/transfers/downloads",
        "token":"SECRET-capability-never-on-disk","expiresAt":"2026-09-20T12:05:00Z",
        "resumeToken":"SECRET-resume-never-on-disk","acceptRanges":true
    })).unwrap()
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("layrs-resume-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn remote(&self) -> NativeRemote {
        NativeRemote::new("https://example.invalid", "SECRET-bearer-never-on-disk")
            .unwrap()
            .with_download_cache(self.0.join("cache"))
    }
    fn spool(&self, name: &str) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(self.0.join(name))
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn durable_prefix_reopens_with_new_capability_and_rejects_corruption() {
    let fixture = Fixture::new();
    let cap = capability();
    let remote = fixture.remote();
    let mut spool = fixture.spool("first");
    let mut resume = remote.resume_download(&cap, &mut spool).unwrap().unwrap();
    spool.write_all(b"exact saved bytes").unwrap();
    resume.save(&mut spool, 17).unwrap();
    let entry = fixture.0.join("cache").join(format!(
        "entry-{}",
        binding(remote.origin.as_str(), &cap).unwrap()
    ));
    let record = entry.join("part-0000");
    let bytes = std::fs::read(&record).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("SECRET"));
    drop(resume);
    drop(remote);
    drop(spool);
    let mut renewed = cap.clone();
    renewed.token = "new authorized capability".into();
    renewed.request_id = RequestId::parse("request:second").unwrap();
    renewed.plan_id = PlanId::parse("plan:second").unwrap();
    renewed.client_id = ClientId::parse("client:after-restart").unwrap();
    let remote = fixture.remote();
    let mut spool = fixture.spool("second");
    let resume = remote
        .resume_download(&renewed, &mut spool)
        .unwrap()
        .unwrap();
    assert_eq!(resume.offset, 17);
    spool.seek(SeekFrom::Start(0)).unwrap();
    let mut got = vec![];
    spool.read_to_end(&mut got).unwrap();
    assert_eq!(got, b"exact saved bytes");
    drop(resume);
    drop(remote);
    drop(spool);
    let mut corrupt = bytes;
    *corrupt.last_mut().unwrap() ^= 1;
    std::fs::write(&record, corrupt).unwrap();
    let mut spool = fixture.spool("corrupt");
    assert!(matches!(
        fixture.remote().resume_download(&renewed, &mut spool),
        Err(NativeRemoteError::Invalid(_))
    ));
}

#[test]
fn binding_is_authority_exact_but_survives_capability_renewal() {
    let cap = capability();
    let original = binding("https://example.invalid/", &cap).unwrap();
    for change in 0..5 {
        let mut other = cap.clone();
        match change {
            0 => other.authorization_epoch += 1,
            1 => other.principal_id = PrincipalId::parse("principal:bob").unwrap(),
            2 => other.space_id = SpaceId::parse("space:other").unwrap(),
            3 => other.descriptor.encoded_byte_len += 1,
            _ => {
                other.token = "renewed".into();
                other.request_id = RequestId::parse("request:new").unwrap();
            }
        }
        assert_eq!(
            original == binding("https://example.invalid/", &other).unwrap(),
            change == 4
        );
    }
    assert_ne!(original, binding("https://other.invalid/", &cap).unwrap());
    let fixture = Fixture::new();
    let mut spool = fixture.spool("old");
    let remote = fixture.remote();
    let mut resume = remote.resume_download(&cap, &mut spool).unwrap().unwrap();
    spool.write_all(b"old").unwrap();
    resume.save(&mut spool, 3).unwrap();
    drop(resume);
    let mut newer = cap;
    newer.authorization_epoch += 1;
    let mut spool = fixture.spool("new");
    let resume = remote.resume_download(&newer, &mut spool).unwrap().unwrap();
    assert_eq!(resume.offset, 0);
}
