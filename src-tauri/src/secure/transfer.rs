//! One transfer actor owns the TLS writer; the connection retains its permanent reader.
use super::*;
use std::collections::HashSet;

pub(super) enum Outgoing {
    Single(Source),
    Bundle(BundleSource),
}
impl Outgoing {
    pub(super) fn name(&self) -> &str {
        match self {
            Self::Single(s) => &s.name,
            Self::Bundle(s) => &s.manifest.name,
        }
    }
    pub(super) fn size(&self) -> u64 {
        match self {
            Self::Single(s) => s.size,
            Self::Bundle(s) => s.manifest.total,
        }
    }
    pub(super) fn file_count(&self) -> u32 {
        match self {
            Self::Single(_) => 1,
            Self::Bundle(s) => s.manifest.file_count(),
        }
    }
    pub(super) fn is_folder(&self) -> bool {
        matches!(self, Self::Bundle(_))
    }
    async fn next(&mut self) -> Result<BundleEvent> {
        match self {
            Self::Single(s) => Ok(match s.next_chunk().await? {
                Some(data) => BundleEvent::Data(data),
                None => BundleEvent::Done,
            }),
            Self::Bundle(s) => s.next().await,
        }
    }
}
enum Offer {
    Single { name: String, size: u64 },
    Bundle(Manifest),
}
enum Incoming {
    Single(Sink),
    Bundle(BundleSink),
}
impl Incoming {
    async fn create(inbox: &std::path::Path, offer: Offer) -> Result<Self> {
        match offer {
            Offer::Single { name, size } => {
                Ok(Self::Single(Sink::create(inbox, &name, size).await?))
            }
            Offer::Bundle(manifest) => Ok(Self::Bundle(BundleSink::create(inbox, manifest).await?)),
        }
    }
    async fn write(&mut self, data: &[u8]) -> Result<()> {
        match self {
            Self::Single(s) => s.write(data).await,
            Self::Bundle(s) => s.write(data).await,
        }
    }
    async fn finish(
        self,
        hash: Option<String>,
        gate: Arc<CommitGate>,
        on_commit: impl FnOnce() + Send + 'static,
    ) -> Result<PathBuf> {
        match (self, hash) {
            (Self::Single(s), Some(hash)) => s.finish_cancellable(&hash, gate, on_commit).await,
            (Self::Bundle(s), None) => s.finish(gate, on_commit).await,
            _ => Err(Error::Protocol),
        }
    }
}
#[path = "resume_transfer.rs"]
mod resumed;

enum Transfer {
    Resume(Box<resumed::Transfer>),
    Idle,
    AwaitAccept {
        id: String,
        source: Outgoing,
        deadline: Instant,
    },
    Sending {
        id: String,
        source: Outgoing,
        deadline: Instant,
    },
    AwaitComplete {
        id: String,
        deadline: Instant,
    },
    IncomingOffer {
        id: String,
        offer: Offer,
        deadline: Instant,
    },
    Receiving {
        id: String,
        sink: Incoming,
        deadline: Instant,
    },
}
impl Transfer {
    fn check_deadline(&self) -> Result<()> {
        if self
            .deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(Error::Expired);
        }
        Ok(())
    }
    fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Idle => None,
            Self::Resume(state) => Some(state.deadline()),
            Self::AwaitAccept { deadline, .. }
            | Self::Sending { deadline, .. }
            | Self::AwaitComplete { deadline, .. }
            | Self::IncomingOffer { deadline, .. }
            | Self::Receiving { deadline, .. } => Some(*deadline),
        }
    }
}
fn transfer_view(s: &Shared, connection: &str, id: &str, status: &str) {
    view(s, connection, |entry| {
        if let Some(transfer) = &mut entry.view.transfer
            && transfer.id == id
        {
            if entry.file_cancelled {
                return;
            }
            transfer.status = status.into();
            if !matches!(status, "sending" | "receiving") {
                transfer.bytes_per_second = None;
                transfer.eta_seconds = None;
            }
        }
    });
}
fn begin_finalization(s: &Shared, connection: &str, id: &str) -> Result<()> {
    transaction(s, |state| {
        let entry = state
            .entries
            .iter_mut()
            .find(|entry| entry.view.id == connection)
            .ok_or(Error::NotReady)?;
        // Cancel and this transition share the state lock. A cancellation that
        // precedes this boundary cannot start a new filesystem commit.
        if *entry.cancel.borrow() || *s.stop.borrow() {
            return Err(Error::Cancelled);
        }
        let transfer = entry.view.transfer.as_mut().ok_or(Error::Protocol)?;
        if transfer.id != id || transfer.direction != "incoming" || transfer.status != "receiving" {
            return Err(Error::Protocol);
        }
        entry.finish_running = true;
        transfer.status = "finalizing".into();
        transfer.bytes_per_second = None;
        transfer.eta_seconds = None;
        Ok(())
    })
}
fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}
fn reserve_offer(
    s: &Shared,
    connection: &str,
    id: &str,
    name: &str,
    size: u64,
    sender: &str,
    manifest: Option<&Manifest>,
) -> Result<bool> {
    transaction(s, |state| {
        let entry = state
            .entries
            .iter_mut()
            .find(|e| e.view.id == connection)
            .ok_or(Error::NotReady)?;
        if entry
            .view
            .transfer
            .as_ref()
            .is_some_and(TransferView::active)
        {
            return Ok(false);
        }
        entry.gate = Arc::new(CommitGate::default());
        entry.file_cancelled = false;
        entry.cancel_notice = None;
        entry.meter = None;
        entry.approval_deadline = Some(Instant::now() + s.file_approval_duration);
        entry.view.transfer = Some(TransferView {
            id: id.into(),
            direction: "incoming".into(),
            name: name.into(),
            sender_name: Some(sender.into()),
            size,
            file_count: manifest.map_or(1, Manifest::file_count),
            is_folder: manifest.is_some(),
            resuming: false,
            transferred: 0,
            bytes_per_second: None,
            eta_seconds: None,
            status: "awaiting_approval".into(),
            path: None,
            error: None,
        });
        Ok(true)
    })
}

pub(super) async fn run<W: tokio::io::AsyncWrite + Unpin>(
    s: &Arc<Shared>,
    connection: &str,
    peer: &str,
    writer: &mut W,
    messages: &mut mpsc::Receiver<Result<Message>>,
    mut actions: mpsc::Receiver<Action>,
) -> Result<()> {
    let (idle, wake) = {
        let state = lock(&s.state);
        let entry = state
            .entries
            .iter()
            .find(|e| e.view.id == connection)
            .ok_or(Error::NotReady)?;
        (entry.frame_idle.clone(), entry.progress_wake.clone())
    };
    let mut framed = TrackedWriter { writer, idle };
    let _progress_task = AbortTask(tokio::spawn(progress::run(
        s.clone(),
        connection.to_owned(),
        wake,
    )));
    let store = get_store(s)?;
    let mut transfer = Transfer::Idle;
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut last_received = Instant::now();
    let mut control_window = Instant::now();
    let mut controls = 0;
    let mut offer_window = Instant::now();
    let mut offers = 0;
    // Never recycle an offer ID within a TLS session: delayed UI responses must
    // not approve different metadata. Bound history by ending a full session.
    let mut seen_offers = HashSet::new();
    loop {
        transfer.check_deadline()?;
        if last_received.elapsed() >= Duration::from_secs(20) {
            return Err(Error::Expired);
        }
        if !store_lock(&store)?.is_trusted(peer) {
            return Err(Error::Revoked);
        }
        let sending = matches!(transfer, Transfer::Sending { .. })
            || matches!(&transfer,Transfer::Resume(state) if state.sending());
        let deadline = transfer.deadline();
        tokio::select! {
            _=tick.tick()=>framed.send(&Message::Ping{}).await?,
            _=tokio::time::sleep_until((last_received+Duration::from_secs(20)).into())=>return Err(Error::Expired),
            _=tokio::time::sleep_until(deadline.unwrap_or_else(|| Instant::now()+s.file_approval_duration).into()), if deadline.is_some()=>return Err(Error::Expired),
            // Select readiness, then await a whole read/write. Selecting directly
            // on next_chunk would let keepalives cancel a partially completed read.
            _=std::future::ready(()), if sending=>{
                if let Transfer::Resume(state)=&mut transfer {
                    state.send_next(&mut framed,messages).await?;
                    tick.reset();last_received=Instant::now();
                    continue;
                }
                let Transfer::Sending { id, source, deadline } = &mut transfer else { unreachable!() };
                match timeout(FILE_ACTIVITY, source.next()).await.map_err(|_|Error::Expired)?? {
                    BundleEvent::Data(data)=>{
                        let count = data.len();
                        framed.send(&Message::FileChunk{id:id.clone(),data}).await?;
                        progress::record(s, connection, count);
                        *deadline=Instant::now()+FILE_ACTIVITY;
                    }
                    BundleEvent::FileEnd{index,hash}=>{
                        framed.send(&Message::BundleFileEnd{id:id.clone(),index,hash}).await?;
                        *deadline=Instant::now()+FILE_ACTIVITY;
                    }
                    BundleEvent::Done=>{
                        let id=id.clone();
                        let finish=match source {Outgoing::Single(source)=>Message::FileFinish{id:id.clone(),hash:source.digest()},Outgoing::Bundle(_)=>Message::BundleEnd{id:id.clone()}};
                        framed.send(&finish).await?;
                        transfer_view(s,connection,&id,"finalizing");
                        transfer=Transfer::AwaitComplete{id,deadline:Instant::now()+FINALIZATION};
                    }
                }
            }
            action=actions.recv()=>{
                transfer.check_deadline()?;
                match action.ok_or(Error::Cancelled)? {
                    Action::SendFile{id,source}=>{
                        if !matches!(transfer,Transfer::Idle) {return Err(Error::Protocol);}
                        let sender_name = lock(&s.local_name).clone();
                        let offer=match source.as_ref() {Outgoing::Single(source)=>Message::FileOffer{id:id.clone(),name:source.name.clone(),size:source.size,sender_name}, Outgoing::Bundle(source)=>Message::BundleOffer{id:id.clone(),manifest:source.manifest.clone(),sender_name}};
                        framed.send(&offer).await?;
                        transfer_view(s,connection,&id,"awaiting_approval");
                        transfer=Transfer::AwaitAccept{id,source:*source,deadline:Instant::now()+s.file_approval_duration};
                    }
                    Action::ReceiveFile{id,accept}=>{
                        if let Transfer::Resume(state)=&mut transfer {
                            if state.id()!=id {return Err(Error::Protocol);}
                            if state.respond(accept,&mut framed,messages).await? {transfer=Transfer::Idle;}
                            tick.reset();last_received=Instant::now();continue;
                        }
                        let previous=std::mem::replace(&mut transfer,Transfer::Idle);
                        let Transfer::IncomingOffer{id:offered,offer,..}=previous else {return Err(Error::Protocol);};
                        if id!=offered {return Err(Error::Protocol);}
                        if accept {
                            let sink=timeout(FILE_ACTIVITY,Incoming::create(&s.receive_directory,offer)).await.map_err(|_|Error::Expired)??;
                            framed.send(&Message::FileAccept{id:id.clone(),accept:true}).await?;
                            transfer_view(s,connection,&id,"receiving");
                            progress::begin(s, connection);
                            transfer=Transfer::Receiving{id,sink,deadline:Instant::now()+FILE_ACTIVITY};
                        }else{
                            framed.send(&Message::FileAccept{id:id.clone(),accept:false}).await?;
                            transfer_view(s,connection,&id,"rejected");
                        }
                    }
                    Action::Resume{id}=>{
                        if !matches!(transfer,Transfer::Idle){return Err(Error::Busy);}
                        let catalog=get_catalog(s)?;let tid=id.clone();let bound=peer.to_owned();
                        let record=tokio::task::spawn_blocking(move||catalog.get(&bound,&tid)).await.map_err(|_|Error::FileIo)??;
                        if record.direction=="incoming" {
                            framed.send(&Message::ResumeRequest{id,digest:record.manifest.digest()}).await?;
                        }else{
                            transfer=Transfer::Resume(Box::new(resumed::Transfer::offer(s,connection,peer,id,&mut framed,messages).await?));
                            tick.reset();last_received=Instant::now();
                        }
                    }
                    Action::Pairing{..}=>return Err(Error::Protocol),
                }
            }
            message=next(messages)=>{
                transfer.check_deadline()?;
                if last_received.elapsed() >= Duration::from_secs(20) {return Err(Error::Expired);}
                let message=message?;
                last_received=Instant::now();
                if !matches!(message,Message::FileChunk{..}|Message::BundleFileEnd{..}|Message::ResumeCheckpoint{..}|Message::ResumeAck{..}|Message::ResumeFileEnd{..}) {
                    if control_window.elapsed()>=Duration::from_secs(60){control_window=Instant::now();controls=0;}
                    controls+=1;if controls>64{return Err(Error::Limit);}
                }
                if matches!(message,Message::ResumeCompleteAck{..}) {
                    let Message::ResumeCompleteAck{id}=message else {unreachable!()};
                    let catalog=get_catalog(s)?;let bound=peer.to_owned();
                    tokio::task::spawn_blocking(move|| {if !catalog.get(&bound,&id)?.completed{return Err(Error::Protocol);}catalog.mark_complete(&bound,&id)}).await.map_err(|_|Error::FileIo)??;
                    refresh_resumable(s).await?;continue;
                }
                if let Transfer::Resume(state)=&mut transfer
                    && matches!(message,Message::FileChunk{..}|Message::ResumeAccept{..}|Message::ResumeCheckpoint{..}|Message::ResumeAck{..}|Message::ResumeFileEnd{..}|Message::ResumeEnd{..}|Message::ResumeComplete{..}) {
                    if state.message(message,&mut framed,messages).await? {transfer=Transfer::Idle;}
                    tick.reset();last_received=Instant::now();continue;
                }
                match message {
                    Message::ResumeRequest{id,digest}=>{
                        if !supports_resume(s,connection) {return Err(Error::Protocol);}
                        let catalog=get_catalog(s)?;let bound=peer.to_owned();let tid=id.clone();
                        let record=tokio::task::spawn_blocking(move||catalog.get(&bound,&tid)).await.map_err(|_|Error::FileIo)??;
                        if record.direction!="outgoing" || record.manifest.digest()!=digest {return Err(Error::Protocol);}
                        lock(&s.requested).insert(id);refresh_resumable(s).await?;
                    }
                    Message::ResumeOffer{id,manifest,sender_name}=>{
                        if !supports_resume(s,connection)||!valid_id(&id)||seen_offers.contains(&id){return Err(Error::Protocol);}
                        validate_sender(&sender_name)?;manifest.validate()?;
                        if seen_offers.len()>=128{return Err(Error::Limit);}seen_offers.insert(id.clone());
                        if offer_window.elapsed()>=Duration::from_secs(60){offer_window=Instant::now();offers=0;}offers+=1;if offers>30{return Err(Error::Limit);}
                        if !matches!(transfer,Transfer::Idle){return Err(Error::Busy);}
                        if let Some(state)=resumed::Transfer::incoming(s,connection,peer,id,manifest,sender_name,&mut framed,messages).await? {transfer=Transfer::Resume(Box::new(state));}
                        tick.reset();last_received=Instant::now();
                    }
                    Message::Ping{}=>framed.send(&Message::Pong{}).await?,
                    Message::Pong{}=>{},
                    Message::FileOffer{id,name,size,sender_name}=>{
                        validate_sender(&sender_name)?;
                        if !valid_id(&id){return Err(Error::Protocol);}
                        if seen_offers.contains(&id){return Err(Error::Protocol);}
                        if seen_offers.len()>=128{return Err(Error::Limit);}
                        seen_offers.insert(id.clone());
                        files::validate_name(&name)?;
                        let bundles = supports_bundles(s,connection);
                        if size>files::MAX_FILE_SIZE || (!bundles && size>files::LEGACY_MAX_FILE_SIZE) {return Err(Error::Limit);}
                        if offer_window.elapsed()>=Duration::from_secs(60){offer_window=Instant::now();offers=0;}
                        offers+=1;if offers>30{return Err(Error::Limit);}
                        if !matches!(transfer,Transfer::Idle)||!reserve_offer(s,connection,&id,&name,size,&sender_name,None)? {
                            framed.send(&Message::FileAccept{id,accept:false}).await?;
                        }else{
                            transfer=Transfer::IncomingOffer{id,offer:Offer::Single{name,size},deadline:Instant::now()+s.file_approval_duration};
                        }
                    }
                    Message::BundleOffer{id,manifest,sender_name}=>{
                        if !supports_bundles(s,connection) {return Err(Error::Protocol);}
                        validate_sender(&sender_name)?; manifest.validate()?;
                        if !valid_id(&id)||seen_offers.contains(&id){return Err(Error::Protocol);}
                        if seen_offers.len()>=128{return Err(Error::Limit);}
                        seen_offers.insert(id.clone());
                        if offer_window.elapsed()>=Duration::from_secs(60){offer_window=Instant::now();offers=0;}
                        offers+=1;if offers>30{return Err(Error::Limit);}
                        if !matches!(transfer,Transfer::Idle)||!reserve_offer(s,connection,&id,&manifest.name,manifest.total,&sender_name,Some(&manifest))? {
                            framed.send(&Message::FileAccept{id,accept:false}).await?;
                        }else{
                            transfer=Transfer::IncomingOffer{id,offer:Offer::Bundle(manifest),deadline:Instant::now()+s.file_approval_duration};
                        }
                    }
                    Message::FileAccept{id,accept}=>{
                        let previous=std::mem::replace(&mut transfer,Transfer::Idle);
                        let Transfer::AwaitAccept{id:offered,source,..}=previous else{return Err(Error::Protocol);};
                        if id!=offered{return Err(Error::Protocol);}
                        if accept {
                            transfer_view(s,connection,&id,"sending");
                            progress::begin(s, connection);
                            transfer=Transfer::Sending{id,source,deadline:Instant::now()+FILE_ACTIVITY};
                        }else{
                            transfer_view(s,connection,&id,"rejected");
                        }
                    }
                    Message::FileChunk{id,data}=>{
                        let Transfer::Receiving{id:expected,sink,deadline}=&mut transfer else{return Err(Error::Protocol);};
                        if &id!=expected{return Err(Error::Protocol);}
                        timeout(FILE_ACTIVITY,sink.write(&data)).await.map_err(|_|Error::Expired)??;
                        progress::record(s, connection, data.len());
                        *deadline=Instant::now()+FILE_ACTIVITY;
                    }
                    Message::BundleFileEnd{id,index,hash}=>{
                        let Transfer::Receiving{id:expected,sink:Incoming::Bundle(sink),deadline}=&mut transfer else {return Err(Error::Protocol);};
                        if &id!=expected {return Err(Error::Protocol);}
                        timeout(FILE_ACTIVITY,sink.file_end(index,&hash)).await.map_err(|_|Error::Expired)??;
                        *deadline=Instant::now()+FILE_ACTIVITY;
                    }
                    finish @ (Message::FileFinish{..}|Message::BundleEnd{..})=>{
                        let (id,hash)=match finish {Message::FileFinish{id,hash}=>(id,Some(hash)),Message::BundleEnd{id}=>(id,None),_=>unreachable!()};
                        let previous=std::mem::replace(&mut transfer,Transfer::Idle);
                        let Transfer::Receiving{id:expected,sink,..}=previous else{return Err(Error::Protocol);};
                        if id!=expected{return Err(Error::Protocol);}
                        if !store_lock(&store)?.is_trusted(peer){return Err(Error::Revoked);}
                        begin_finalization(s,connection,&id)?;
                        let shared=s.clone();let cid=connection.to_owned();let finished=id.clone();
                        // A verified filesystem commit may outlive cancellation.
                        // Its owner always publishes the actual completion/error.
                        let gate = {
                            let state=lock(&s.state);
                            state.entries.iter().find(|e|e.view.id==connection).ok_or(Error::NotReady)?.gate.clone()
                        };
                        let (done, result) = tokio::sync::oneshot::channel();
                        let finalize=tokio::spawn(async move {
                            let committing_shared=shared.clone(); let committing_cid=cid.clone(); let committing_id=finished.clone();
                            let result=sink.finish(hash, gate, move || transfer_view(&committing_shared,&committing_cid,&committing_id,"committing")).await;
                            view(&shared,&cid,|entry|{
                                entry.finish_running=false;
                                if let Some(transfer)=&mut entry.view.transfer && transfer.id==finished {
                                    transfer.bytes_per_second=None; transfer.eta_seconds=None;
                                    match &result {
                                        Ok(path)=>{transfer.status="complete".into();transfer.path=Some(path.to_string_lossy().into_owned());transfer.error=None;}
                                        Err(_) if entry.file_cancelled=>{transfer.status="cancelled".into();transfer.error=None;}
                                        Err(error)=>{transfer.status="error".into();transfer.error=entry.transport_error.clone().or_else(||Some(error.to_string()));}
                                    }
                                }
                            });
                            let _=done.send(result);
                        });
                        lock(&s.tasks).push(finalize);
                        wait_finalization(&mut framed,messages,result,&mut last_received,&mut control_window,&mut controls).await?;
                        // The helper sent keepalives during fsync. Discard elapsed
                        // outer ticks instead of bursting them at the peer now.
                        tick.reset();
                        framed.send(&Message::FileComplete{id}).await?;
                    }
                    Message::FileComplete{id}=>{
                        let Transfer::AwaitComplete{id:expected,..}=&transfer else{return Err(Error::Protocol);};
                        if &id!=expected{return Err(Error::Protocol);}
                        transfer_view(s,connection,&id,"complete");
                        transfer=Transfer::Idle;
                    }
                    _=>return Err(Error::Protocol),
                }
            }
        }
    }
}

fn validate_sender(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name.len() > 80
        || name.chars().any(|c| {
            c.is_control()
                || matches!(c as u32, 0x061c | 0x200e..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
        })
    {
        return Err(Error::Protocol);
    }
    Ok(())
}

struct TrackedWriter<'a, W> {
    writer: &'a mut W,
    idle: Arc<std::sync::atomic::AtomicBool>,
}
impl<W: tokio::io::AsyncWrite + Unpin> TrackedWriter<'_, W> {
    async fn send(&mut self, message: &Message) -> Result<()> {
        self.idle.store(false, Ordering::SeqCst);
        let result = protocol::send(self.writer, message).await;
        if result.is_ok() {
            self.idle.store(true, Ordering::SeqCst);
        }
        result
    }
}

fn supports_bundles(s: &Shared, connection: &str) -> bool {
    lock(&s.state)
        .entries
        .iter()
        .find(|e| e.view.id == connection)
        .is_some_and(|e| e.view.supports_bundles)
}

// Large-file fsync may take longer than an idle interval. Keep the authenticated
// channel responsive while the owned finalizer retains the commit/cancel gate.
const FINALIZATION: Duration = Duration::from_secs(5 * 60);
async fn wait_finalization<W: tokio::io::AsyncWrite + Unpin>(
    framed: &mut TrackedWriter<'_, W>,
    messages: &mut mpsc::Receiver<Result<Message>>,
    result: tokio::sync::oneshot::Receiver<Result<PathBuf>>,
    last_received: &mut Instant,
    window: &mut Instant,
    controls: &mut u32,
) -> Result<PathBuf> {
    tokio::pin!(result);
    let deadline = tokio::time::sleep(FINALIZATION);
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            finished=&mut result=>return finished.map_err(|_|Error::FileIo)?,
            _=&mut deadline=>return Err(Error::Expired),
            _=tokio::time::sleep_until((*last_received+Duration::from_secs(20)).into())=>return Err(Error::Expired),
            _=tick.tick()=>framed.send(&Message::Ping{}).await?,
            message=next(messages)=>{
                if window.elapsed()>=Duration::from_secs(60){*window=Instant::now();*controls=0;}
                *controls+=1;if *controls>64{return Err(Error::Limit);}
                *last_received=Instant::now();
                match message? {Message::Ping{}=>framed.send(&Message::Pong{}).await?, Message::Pong{}=>{}, _=>return Err(Error::Protocol)}
            }
        }
    }
}

fn supports_resume(s: &Shared, connection: &str) -> bool {
    lock(&s.state)
        .entries
        .iter()
        .find(|e| e.view.id == connection)
        .is_some_and(|e| e.view.supports_resume)
}
