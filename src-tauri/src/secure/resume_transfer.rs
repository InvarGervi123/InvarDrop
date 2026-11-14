//! Resumable transfers share the connection's single framed writer. Persistent
//! IO runs on blocking workers; dropping a connection retains durable prefixes.
use super::super::resume::{self, Catalog, Position, Record};
use super::*;

const NEGOTIATION: Duration = Duration::from_secs(5 * 60);
type Sending = Arc<Mutex<resume::Sender>>;
type Receiving = Arc<Mutex<Option<resume::Receiver>>>;
enum Phase {
    AwaitAccept,
    Offered,
    Sending(Sending),
    AwaitAck { sender: Sending, position: Position },
    Receiving(Receiving),
    AwaitComplete,
    Finished,
}
pub(super) struct Transfer {
    pub(super) id: String,
    shared: Arc<Shared>,
    connection: String,
    peer: String,
    catalog: Arc<Catalog>,
    manifest: resume::Manifest,
    phase: Phase,
    deadline: Instant,
}
impl Transfer {
    pub(super) fn id(&self) -> &str {
        &self.id
    }
    pub(super) fn sending(&self) -> bool {
        matches!(self.phase, Phase::Sending(_))
    }
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(super) async fn offer<W: tokio::io::AsyncWrite + Unpin>(
        s: &Arc<Shared>,
        connection: &str,
        peer: &str,
        id: String,
        framed: &mut TrackedWriter<'_, W>,
        messages: &mut mpsc::Receiver<Result<Message>>,
    ) -> Result<Self> {
        let catalog = get_catalog(s)?;
        let work = {
            let catalog = catalog.clone();
            let peer = peer.to_owned();
            let id = id.clone();
            move || catalog.get(&peer, &id)
        };
        let record = negotiate(s, framed, messages, work).await?;
        if record.direction != "outgoing" || record.completed {
            return Err(Error::Protocol);
        }
        reserve(
            s,
            connection,
            peer,
            &record.id,
            &record.manifest,
            ViewOffer {
                sender: None,
                verified: record.verified(),
                resuming: record.verified() > 0,
            },
        )?;
        let sender_name = lock(&s.local_name).clone();
        framed
            .send(&Message::ResumeOffer {
                id: id.clone(),
                sender_name,
                manifest: record.manifest.clone(),
            })
            .await?;
        Ok(Self {
            id,
            shared: s.clone(),
            connection: connection.into(),
            peer: peer.into(),
            catalog,
            manifest: record.manifest,
            phase: Phase::AwaitAccept,
            deadline: Instant::now() + s.file_approval_duration,
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn incoming<W: tokio::io::AsyncWrite + Unpin>(
        s: &Arc<Shared>,
        connection: &str,
        peer: &str,
        id: String,
        manifest: resume::Manifest,
        sender_name: String,
        framed: &mut TrackedWriter<'_, W>,
        messages: &mut mpsc::Receiver<Result<Message>>,
    ) -> Result<Option<Self>> {
        validate_sender(&sender_name)?;
        manifest.validate()?;
        if !valid_id(&id) {
            return Err(Error::Protocol);
        }
        let catalog = get_catalog(s)?;
        let work = {
            let catalog = catalog.clone();
            let peer = peer.to_owned();
            let id = id.clone();
            move || match catalog.get(&peer, &id) {
                Ok(record) => Ok(Some(record)),
                Err(Error::FileIo) => Ok(None),
                Err(error) => Err(error),
            }
        };
        let previous = negotiate(s, framed, messages, work).await?;
        if let Some(record) = &previous
            && (record.direction != "incoming" || record.manifest.digest() != manifest.digest())
        {
            return Err(Error::Integrity);
        }
        let verified = previous.as_ref().map_or(0, Record::verified);
        if previous.as_ref().is_some_and(|r| r.completed) {
            let work = {
                let catalog = catalog.clone();
                let peer = peer.to_owned();
                let id = id.clone();
                move || catalog.completed(&peer, &id)?.ok_or(Error::Integrity)
            };
            let path = negotiate(s, framed, messages, work).await?;
            reserve(
                s,
                connection,
                peer,
                &id,
                &manifest,
                ViewOffer {
                    sender: Some(&sender_name),
                    verified: manifest.tree.total,
                    resuming: true,
                },
            )?;
            completed(s, connection, &id, &path);
            framed.send(&Message::ResumeComplete { id }).await?;
            return Ok(None);
        }
        reserve(
            s,
            connection,
            peer,
            &id,
            &manifest,
            ViewOffer {
                sender: Some(&sender_name),
                verified,
                resuming: previous.is_some(),
            },
        )?;
        Ok(Some(Self {
            id,
            shared: s.clone(),
            connection: connection.into(),
            peer: peer.into(),
            catalog,
            manifest,
            phase: Phase::Offered,
            deadline: Instant::now() + s.file_approval_duration,
        }))
    }
    pub(super) async fn respond<W: tokio::io::AsyncWrite + Unpin>(
        &mut self,
        accept: bool,
        framed: &mut TrackedWriter<'_, W>,
        messages: &mut mpsc::Receiver<Result<Message>>,
    ) -> Result<bool> {
        if !matches!(self.phase, Phase::Offered) {
            return Err(Error::Protocol);
        }
        if !accept {
            framed
                .send(&Message::ResumeAccept {
                    id: self.id.clone(),
                    accept: false,
                    position: start(&self.manifest),
                })
                .await?;
            transfer_view(&self.shared, &self.connection, &self.id, "rejected");
            self.phase = Phase::Finished;
            return Ok(true);
        }
        transfer_view(&self.shared, &self.connection, &self.id, "preparing");
        let work = {
            let catalog = self.catalog.clone();
            let peer = self.peer.clone();
            let id = self.id.clone();
            let manifest = self.manifest.clone();
            let gate = self.gate()?;
            move || catalog.accept_cancellable(&peer, &id, manifest, gate)
        };
        let receiver = negotiate(&self.shared, framed, messages, work).await?;
        let position = receiver.position();
        let verified = receiver.verified();
        self.begin_progress(verified, "receiving");
        self.phase = Phase::Receiving(Arc::new(Mutex::new(Some(receiver))));
        framed
            .send(&Message::ResumeAccept {
                id: self.id.clone(),
                accept: true,
                position,
            })
            .await?;
        // The sender validates all source hashes before starting payload IO.
        self.deadline = Instant::now() + NEGOTIATION;
        Ok(false)
    }
    pub(super) async fn send_next<W: tokio::io::AsyncWrite + Unpin>(
        &mut self,
        framed: &mut TrackedWriter<'_, W>,
        _messages: &mut mpsc::Receiver<Result<Message>>,
    ) -> Result<()> {
        let Phase::Sending(sender) = &self.phase else {
            return Err(Error::Protocol);
        };
        let sender = sender.clone();
        let work = sender.clone();
        match blocking(move || lock(&work).next()).await? {
            resume::Event::Data(data) => {
                let count = data.len();
                framed
                    .send(&Message::FileChunk {
                        id: self.id.clone(),
                        data,
                    })
                    .await?;
                progress::record(&self.shared, &self.connection, count);
                self.deadline = Instant::now() + FILE_ACTIVITY;
            }
            resume::Event::Checkpoint(position) => {
                framed
                    .send(&Message::ResumeCheckpoint {
                        id: self.id.clone(),
                        position: position.clone(),
                    })
                    .await?;
                self.phase = Phase::AwaitAck { sender, position };
                self.deadline = Instant::now() + NEGOTIATION;
            }
            resume::Event::FileEnd { index, hash } => {
                let position = lock(&sender).position();
                framed
                    .send(&Message::ResumeFileEnd {
                        id: self.id.clone(),
                        index,
                        hash,
                    })
                    .await?;
                self.phase = Phase::AwaitAck { sender, position };
                self.deadline = Instant::now() + NEGOTIATION;
            }
            resume::Event::Done => {
                self.phase = Phase::AwaitComplete;
                transfer_view(&self.shared, &self.connection, &self.id, "finalizing");
                framed
                    .send(&Message::ResumeEnd {
                        id: self.id.clone(),
                    })
                    .await?;
                self.deadline = Instant::now() + NEGOTIATION;
            }
        }
        Ok(())
    }
    pub(super) async fn message<W: tokio::io::AsyncWrite + Unpin>(
        &mut self,
        message: Message,
        framed: &mut TrackedWriter<'_, W>,
        messages: &mut mpsc::Receiver<Result<Message>>,
    ) -> Result<bool> {
        match message {
            Message::ResumeAccept {
                id,
                accept,
                position,
            } => {
                self.check_id(&id)?;
                if !matches!(self.phase, Phase::AwaitAccept) {
                    return Err(Error::Protocol);
                }
                if !accept {
                    transfer_view(&self.shared, &self.connection, &id, "rejected");
                    self.phase = Phase::Finished;
                    return Ok(true);
                }
                transfer_view(&self.shared, &self.connection, &id, "preparing");
                let work = {
                    let catalog = self.catalog.clone();
                    let peer = self.peer.clone();
                    let id = id.clone();
                    let position = position.clone();
                    let gate = self.gate()?;
                    move || catalog.sender_cancellable(&peer, &id, position, gate)
                };
                let sender = negotiate(&self.shared, framed, messages, work).await?;
                self.begin_progress(self.manifest.verified(&position), "sending");
                self.phase = Phase::Sending(Arc::new(Mutex::new(sender)));
                self.deadline = Instant::now() + FILE_ACTIVITY;
            }
            Message::FileChunk { id, data } => {
                self.check_id(&id)?;
                let receiver = self.receiver()?;
                let count = data.len();
                blocking(move || {
                    lock(&receiver)
                        .as_mut()
                        .ok_or(Error::Protocol)?
                        .write(&data)
                })
                .await?;
                progress::record(&self.shared, &self.connection, count);
                self.deadline = Instant::now() + FILE_ACTIVITY;
            }
            Message::ResumeCheckpoint { id, position } => {
                self.check_id(&id)?;
                let receiver = self.receiver()?;
                let saved = position.clone();
                negotiate(&self.shared, framed, messages, move || {
                    lock(&receiver)
                        .as_mut()
                        .ok_or(Error::Protocol)?
                        .checkpoint(saved)
                })
                .await?;
                framed.send(&Message::ResumeAck { id, position }).await?;
                self.deadline = Instant::now() + FILE_ACTIVITY;
            }
            Message::ResumeFileEnd { id, index, hash } => {
                self.check_id(&id)?;
                let receiver = self.receiver()?;
                let position = negotiate(&self.shared, framed, messages, move || {
                    let mut receiver = lock(&receiver);
                    let receiver = receiver.as_mut().ok_or(Error::Protocol)?;
                    receiver.file_end(index, &hash)?;
                    Ok(receiver.position())
                })
                .await?;
                framed.send(&Message::ResumeAck { id, position }).await?;
                self.deadline = Instant::now() + FILE_ACTIVITY;
            }
            Message::ResumeAck { id, position } => {
                self.check_id(&id)?;
                let phase = std::mem::replace(&mut self.phase, Phase::Finished);
                let Phase::AwaitAck {
                    sender,
                    position: expected,
                } = phase
                else {
                    return Err(Error::Protocol);
                };
                if position != expected {
                    return Err(Error::Integrity);
                }
                let catalog = self.catalog.clone();
                let peer = self.peer.clone();
                blocking(move || catalog.note_position(&peer, &id, position)).await?;
                self.phase = Phase::Sending(sender);
                self.deadline = Instant::now() + FILE_ACTIVITY;
            }
            Message::ResumeEnd { id } => {
                self.check_id(&id)?;
                let receiver = self.receiver()?;
                begin_finalization(&self.shared, &self.connection, &id)?;
                let gate = {
                    let state = lock(&self.shared.state);
                    state
                        .entries
                        .iter()
                        .find(|e| e.view.id == self.connection)
                        .ok_or(Error::NotReady)?
                        .gate
                        .clone()
                };
                let shared = self.shared.clone();
                let connection = self.connection.clone();
                let finished = id.clone();
                let (done, result) = tokio::sync::oneshot::channel();
                let task = tokio::spawn(async move {
                    let committing_shared = shared.clone();
                    let committing_connection = connection.clone();
                    let committing_id = finished.clone();
                    let result = blocking(move || {
                        let receiver = lock(&receiver).take().ok_or(Error::Protocol)?;
                        receiver.finish(gate, move || {
                            transfer_view(
                                &committing_shared,
                                &committing_connection,
                                &committing_id,
                                "committing",
                            )
                        })
                    })
                    .await;
                    view(&shared, &connection, |entry| {
                        entry.finish_running = false;
                        if let Some(transfer) = &mut entry.view.transfer
                            && transfer.id == finished
                        {
                            transfer.bytes_per_second = None;
                            transfer.eta_seconds = None;
                            match &result {
                                Ok(path) => {
                                    transfer.status = "complete".into();
                                    transfer.path = Some(path.to_string_lossy().into_owned());
                                    transfer.error = None;
                                }
                                Err(_) if entry.file_cancelled => {
                                    transfer.status = "cancelled".into();
                                    transfer.error = None;
                                }
                                Err(error) => {
                                    transfer.status = "error".into();
                                    transfer.error = entry
                                        .transport_error
                                        .clone()
                                        .or_else(|| Some(error.to_string()));
                                }
                            }
                        }
                    });
                    let _ = refresh_resumable(&shared).await;
                    let _ = done.send(result);
                });
                lock(&self.shared.tasks).push(task);
                wait_result(framed, messages, result).await?;
                self.phase = Phase::Finished;
                framed.send(&Message::ResumeComplete { id }).await?;
                let _ = refresh_resumable(&self.shared).await;
                return Ok(true);
            }
            Message::ResumeComplete { id } => {
                self.check_id(&id)?;
                if !matches!(self.phase, Phase::AwaitAccept | Phase::AwaitComplete) {
                    return Err(Error::Protocol);
                }
                // Release the source lease before deleting its durable ticket.
                self.phase = Phase::Finished;
                let catalog = self.catalog.clone();
                let peer = self.peer.clone();
                let finished = id.clone();
                blocking(move || catalog.mark_complete(&peer, &finished)).await?;
                transfer_view(&self.shared, &self.connection, &id, "complete");
                framed.send(&Message::ResumeCompleteAck { id }).await?;
                let _ = refresh_resumable(&self.shared).await;
                return Ok(true);
            }
            _ => return Err(Error::Protocol),
        }
        Ok(false)
    }
    fn gate(&self) -> Result<Arc<CommitGate>> {
        let state = lock(&self.shared.state);
        state
            .entries
            .iter()
            .find(|entry| {
                entry.view.id == self.connection
                    && entry
                        .view
                        .transfer
                        .as_ref()
                        .is_some_and(|transfer| transfer.id == self.id)
            })
            .map(|entry| entry.gate.clone())
            .ok_or(Error::NotReady)
    }
    fn receiver(&self) -> Result<Receiving> {
        match &self.phase {
            Phase::Receiving(receiver) => Ok(receiver.clone()),
            _ => Err(Error::Protocol),
        }
    }
    fn check_id(&self, id: &str) -> Result<()> {
        if self.id == id {
            Ok(())
        } else {
            Err(Error::Protocol)
        }
    }
    fn begin_progress(&self, verified: u64, status: &str) {
        view(&self.shared, &self.connection, |entry| {
            if let Some(transfer) = &mut entry.view.transfer
                && transfer.id == self.id
            {
                transfer.transferred = verified;
                transfer.status = status.into();
                transfer.bytes_per_second = None;
                transfer.eta_seconds = None;
            }
        });
        progress::begin(&self.shared, &self.connection);
    }
}
struct ViewOffer<'a> {
    sender: Option<&'a str>,
    verified: u64,
    resuming: bool,
}
fn reserve(
    s: &Shared,
    connection: &str,
    peer: &str,
    id: &str,
    manifest: &resume::Manifest,
    offer: ViewOffer<'_>,
) -> Result<()> {
    let ViewOffer {
        sender,
        verified,
        resuming,
    } = offer;
    transaction(s, |state| {
        let entry = state
            .entries
            .iter_mut()
            .find(|e| e.view.id == connection)
            .ok_or(Error::NotReady)?;
        if entry.view.phase != "connected"
            || entry.view.peer.as_deref() != Some(peer)
            || !entry.view.supports_resume
            || *entry.cancel.borrow()
            || *s.stop.borrow()
            || entry.view.transfer.as_ref().is_some_and(|transfer| {
                transfer.active()
                    && !(sender.is_none()
                        && transfer.id == id
                        && transfer.direction == "outgoing"
                        && transfer.status == "offering")
            })
        {
            return Err(Error::NotReady);
        }
        entry.gate = Arc::new(CommitGate::default());
        entry.file_cancelled = false;
        entry.cancel_notice = None;
        entry.finish_running = false;
        entry.meter = None;
        entry.approval_deadline = sender.map(|_| Instant::now() + s.file_approval_duration);
        entry.view.transfer = Some(TransferView {
            id: id.into(),
            direction: if sender.is_some() {
                "incoming"
            } else {
                "outgoing"
            }
            .into(),
            name: manifest.tree.name.clone(),
            sender_name: sender.map(str::to_owned),
            size: manifest.tree.total,
            file_count: manifest.tree.file_count(),
            is_folder: manifest.folder,
            resuming,
            transferred: verified,
            bytes_per_second: None,
            eta_seconds: None,
            status: "awaiting_approval".into(),
            path: None,
            error: None,
        });
        Ok(())
    })
}
fn completed(s: &Shared, connection: &str, id: &str, path: &std::path::Path) {
    view(s, connection, |entry| {
        if let Some(transfer) = &mut entry.view.transfer
            && transfer.id == id
        {
            transfer.status = "complete".into();
            transfer.transferred = transfer.size;
            transfer.path = Some(path.to_string_lossy().into_owned());
        }
    });
}
fn start(manifest: &resume::Manifest) -> Position {
    Position {
        entry: manifest
            .tree
            .entries
            .iter()
            .position(|e| e.kind == crate::secure::bundle::Kind::File)
            .unwrap_or(manifest.tree.entries.len()) as u32,
        offset: 0,
        hash: blake3::hash(b"").to_hex().to_string(),
    }
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Error::FileIo)?
}
async fn negotiate<T: Send + 'static, W: tokio::io::AsyncWrite + Unpin>(
    s: &Arc<Shared>,
    framed: &mut TrackedWriter<'_, W>,
    messages: &mut mpsc::Receiver<Result<Message>>,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let (done, result) = tokio::sync::oneshot::channel();
    let shared = s.clone();
    let task = tokio::spawn(async move {
        if let Err(result) = done.send(blocking(work).await) {
            drop(result); // release any detached Sender/Receiver lease first
            let _ = refresh_resumable(&shared).await;
        }
    });
    {
        let mut tasks = lock(&s.tasks);
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
    }
    wait_result(framed, messages, result).await
}
async fn wait_result<T: Send + 'static, W: tokio::io::AsyncWrite + Unpin>(
    framed: &mut TrackedWriter<'_, W>,
    messages: &mut mpsc::Receiver<Result<Message>>,
    result: tokio::sync::oneshot::Receiver<Result<T>>,
) -> Result<T> {
    tokio::pin!(result);
    let deadline = tokio::time::sleep(NEGOTIATION);
    tokio::pin!(deadline);
    // Short checkpoint/file-end IO must not generate an immediate Ping each:
    // hundreds of tiny files would otherwise exhaust the peer's control quota.
    let mut tick = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(5),
        Duration::from_secs(5),
    );
    let mut last = Instant::now();
    let mut window = last;
    let mut controls = 0;
    loop {
        tokio::select! {
            result=&mut result=>return result.map_err(|_|Error::FileIo)?,
            _=&mut deadline=>return Err(Error::Expired),
            _=tokio::time::sleep_until((last+Duration::from_secs(20)).into())=>return Err(Error::Expired),
            _=tick.tick()=>framed.send(&Message::Ping{}).await?,
            message=next(messages)=>{
                if window.elapsed()>=Duration::from_secs(60){window=Instant::now();controls=0;}
                controls+=1;if controls>64{return Err(Error::Limit);}last=Instant::now();
                match message? {Message::Ping{}=>framed.send(&Message::Pong{}).await?,Message::Pong{}=>{},_=>return Err(Error::Protocol)}
            }
        }
    }
}
