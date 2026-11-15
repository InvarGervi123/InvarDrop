use super::*;
async fn connect(
    service: &Service,
    identity: &Identity,
) -> (tokio_rustls::TlsStream<TcpStream>, Connection) {
    let peer = identity.fingerprint().unwrap();
    store_lock(&get_store(&service.shared).unwrap())
        .unwrap()
        .trust(&peer)
        .unwrap();
    refresh_trust(&service.shared).unwrap();
    let connector = transport::connector(identity, service.snapshot().identity).unwrap();
    let tcp = TcpStream::connect(addr(service)).await.unwrap();
    let mut tls = connector
        .connect("invardrop.local".try_into().unwrap(), tcp)
        .await
        .unwrap()
        .into();
    protocol::send(
        &mut tls,
        &Message::Hello {
            version: 6,
            initiator: true,
            trusted: true,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::Hello { version: 6, .. }
    ));
    assert!(matches!(raw_message(&mut tls).await, Message::Ready {}));
    protocol::send(&mut tls, &Message::Ready {}).await.unwrap();
    (tls, phase(service, "connected").await)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_folder_resumes_after_restart_without_reconversion_or_original_changes() {
    let d = tempfile::tempdir().unwrap();
    let master = random::<32>().unwrap();
    let profile = d.path().join("profile");
    let originals = d.path().join("Originals");
    std::fs::create_dir_all(originals.join("nested")).unwrap();
    let binary =
        PathBuf::from(std::env::var_os("INVARDROP_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into()));
    let song = originals.join("song.ogg");
    assert!(
        std::process::Command::new(&binary)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2",
                "-c:a",
                "libvorbis"
            ])
            .arg(&song)
            .status()
            .unwrap()
            .success()
    );
    std::fs::copy(&song, originals.join("nested/second.ogg")).unwrap();
    let original = std::fs::read(&song).unwrap();
    let large = originals.join("0data");
    std::fs::File::create(&large)
        .unwrap()
        .set_len(17 * 1024 * 1024)
        .unwrap();
    let source = BundleSource::folder(&originals).await.unwrap();
    let prepared = d.path().join("Prepared");
    crate::processing::convert_tree(
        &binary,
        &source.manifest,
        &source.paths(),
        &prepared,
        true,
        &CommitGate::default(),
        || {},
    )
    .unwrap();
    assert!(prepared.join("song.mp3").exists());
    assert!(prepared.join("nested/second.mp3").exists());
    assert_eq!(std::fs::read(&song).unwrap(), original);
    let source = start_with_master(&profile, Duration::from_secs(10), &master).await;
    let identity = Identity::generate().unwrap();
    let (mut tls, c) = connect(&source, &identity).await;
    source
        .send_resumable_bundle(&c.id, BundleSource::folder(&prepared).await.unwrap())
        .await
        .unwrap();
    let (id, manifest) = match raw_message(&mut tls).await {
        Message::ResumeOffer { id, manifest, .. } => (id, manifest),
        _ => panic!("Expected offer"),
    };
    let empty = blake3::hash(b"").to_hex().to_string();
    protocol::send(
        &mut tls,
        &Message::ResumeAccept {
            id: id.clone(),
            accept: true,
            position: resume::Position {
                entry: 0,
                offset: 0,
                hash: empty.clone(),
            },
        },
    )
    .await
    .unwrap();
    let mut hashes: Vec<_> = manifest
        .tree
        .entries
        .iter()
        .map(|_| blake3::Hasher::new())
        .collect();
    let mut bytes = 0u64;
    let position = loop {
        match raw_message(&mut tls).await {
            Message::FileChunk { data, .. } => {
                bytes += data.len() as u64;
                hashes[0].update(&data);
            }
            Message::ResumeCheckpoint { position, .. } => {
                assert_eq!(bytes, 16 * 1024 * 1024);
                break position;
            }
            _ => panic!("Expected checkpoint"),
        }
    };
    protocol::send(
        &mut tls,
        &Message::ResumeAck {
            id: id.clone(),
            position: position.clone(),
        },
    )
    .await
    .unwrap();
    drop(tls);
    closed(&source, &c.id).await;
    source.stop().await;
    drop(source);
    let source = start_with_master(&profile, Duration::from_secs(10), &master).await;
    let (mut tls, c) = connect(&source, &identity).await;
    source.resume_transfer(&c.id, &id).await.unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::ResumeOffer { .. }
    ));
    protocol::send(
        &mut tls,
        &Message::ResumeAccept {
            id: id.clone(),
            accept: true,
            position,
        },
    )
    .await
    .unwrap();
    let mut index = 0usize;
    let mut remaining = 0;
    loop {
        match raw_message(&mut tls).await {
            Message::FileChunk { data, .. } => {
                remaining += data.len() as u64;
                hashes[index].update(&data);
            }
            Message::ResumeCheckpoint { position, .. } => protocol::send(
                &mut tls,
                &Message::ResumeAck {
                    id: id.clone(),
                    position,
                },
            )
            .await
            .unwrap(),
            Message::ResumeFileEnd {
                index: finished,
                hash,
                ..
            } => {
                assert_eq!(index, finished as usize);
                assert_eq!(hashes[index].finalize().to_hex().as_str(), hash);
                assert_eq!(hash, manifest.hashes[index]);
                index += 1;
                while manifest
                    .tree
                    .entries
                    .get(index)
                    .is_some_and(|e| e.kind == crate::secure::bundle::Kind::Directory)
                {
                    index += 1;
                }
                protocol::send(
                    &mut tls,
                    &Message::ResumeAck {
                        id: id.clone(),
                        position: resume::Position {
                            entry: index as u32,
                            offset: 0,
                            hash: empty.clone(),
                        },
                    },
                )
                .await
                .unwrap();
            }
            Message::ResumeEnd { .. } => {
                protocol::send(&mut tls, &Message::ResumeComplete { id: id.clone() })
                    .await
                    .unwrap();
                break;
            }
            _ => panic!("Unexpected frame"),
        }
    }
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::ResumeCompleteAck { .. }
    ));
    file_status(&source, &c.id, "complete").await;
    assert_eq!(remaining, manifest.tree.total - 16 * 1024 * 1024);
    assert_eq!(std::fs::read(&song).unwrap(), original);
    source.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_conversion_decision_and_prepared_references_preserve_resume_sources() {
    let d = tempfile::tempdir().unwrap();
    let da = d.path().join("a");
    let db = d.path().join("b");
    let a = start(&da, Duration::from_secs(10)).await;
    let b = start(&db, Duration::from_secs(10)).await;
    let (ac, bc) = pair(&a, &b).await;
    let executable = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let binary = std::env::var_os("INVARDROP_TEST_FFMPEG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|p| p.join(executable))
                    .find(|p| p.is_file())
            })
        })
        .unwrap()
        .canonicalize()
        .unwrap();
    let song = d.path().join("song.ogg");
    assert!(
        std::process::Command::new(&binary)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2",
                "-c:a",
                "libvorbis"
            ])
            .arg(&song)
            .status()
            .unwrap()
            .success()
    );
    let original = std::fs::read(&song).unwrap();
    let processing = crate::processing::Processing::default();
    assert!(
        processing
            .select(&a, ac.id.clone(), vec![song.clone()], false)
            .await
            .unwrap()
    );
    assert!(
        a.snapshot()
            .connections
            .iter()
            .all(|c| c.transfer.is_none())
    );
    let id = processing.views()[0].id.clone();
    assert!(processing.decide(&a, id.clone(), true).await.is_err());
    assert_eq!(processing.views()[0].state, "choice");
    processing.decide(&a, id, false).await.unwrap();
    let offer = file_status(&b, &bc.id, "awaiting_approval").await;
    assert_eq!(offer.name, "song.ogg");
    assert_eq!(offer.size, original.len() as u64);
    b.respond_file(&bc.id, &offer.id, true).unwrap();
    let received = file_status(&b, &bc.id, "complete").await;
    file_status(&a, &ac.id, "complete").await;
    assert_eq!(std::fs::read(received.path.unwrap()).unwrap(), original);
    processing.configure(binary).unwrap();
    processing
        .select(&a, ac.id.clone(), vec![song.clone()], false)
        .await
        .unwrap();
    let id = processing.views().last().unwrap().id.clone();
    processing.decide(&a, id.clone(), true).await.unwrap();
    let offer = file_status(&b, &bc.id, "awaiting_approval").await;
    assert_eq!(offer.name, "song.mp3");
    let prepared = std::fs::read_dir(da.join("prepared"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mp3 = prepared.join("song.ogg/song.mp3");
    assert_eq!(std::fs::metadata(&mp3).unwrap().len(), offer.size);
    a.cleanup_prepared().await.unwrap();
    assert!(mp3.exists());
    processing.dismiss(&id).unwrap();
    a.cancel_file(&ac.id, &offer.id).unwrap();
    closed(&a, &ac.id).await;
    a.cleanup_prepared().await.unwrap();
    assert!(
        mp3.exists(),
        "Cancelled transfer still has a durable source ticket"
    );
    a.discard_resume(&offer.id).await.unwrap();
    a.cleanup_prepared().await.unwrap();
    assert!(!prepared.exists());
    assert_eq!(std::fs::read(song).unwrap(), original);
    a.stop().await;
    b.stop().await;
}
