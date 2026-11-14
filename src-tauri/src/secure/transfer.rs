//! One transfer actor owns the TLS writer; the connection retains its permanent reader.
use super::*;
use std::collections::HashSet;

enum Transfer {
    Idle,
    AwaitAccept {
        id: String,
        source: Source,
        deadline: Instant,
    },
    Sending {
        id: String,
        source: Source,
        deadline: Instant,
    },
    AwaitComplete {
        id: String,
        deadline: Instant,
    },
    IncomingOffer {
        id: String,
        name: String,
        size: u64,
        deadline: Instant,
    },
    Receiving {
        id: String,
        sink: Sink,
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
            transfer.status = status.into();
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
        transfer.status = "finalizing".into();
        Ok(())
    })
}
fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}
fn reserve_offer(s: &Shared, connection: &str, id: &str, name: &str, size: u64) -> Result<bool> {
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
        entry.view.transfer = Some(TransferView {
            id: id.into(),
            direction: "incoming".into(),
            name: name.into(),
            size,
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
        let sending = matches!(transfer, Transfer::Sending { .. });
        let deadline = transfer.deadline();
        tokio::select! {
            _=tick.tick()=>protocol::send(writer,&Message::Ping{}).await?,
            _=tokio::time::sleep_until((last_received+Duration::from_secs(20)).into())=>return Err(Error::Expired),
            _=tokio::time::sleep_until(deadline.unwrap_or_else(|| Instant::now()+FILE_APPROVAL).into()), if deadline.is_some()=>return Err(Error::Expired),
            // Select readiness, then await a whole read/write. Selecting directly
            // on next_chunk would let keepalives cancel a partially completed read.
            _=std::future::ready(()), if sending=>{
                let Transfer::Sending { id, source, deadline } = &mut transfer else { unreachable!() };
                match timeout(FILE_ACTIVITY, source.next_chunk()).await.map_err(|_|Error::Expired)?? {
                    Some(data)=>{
                        protocol::send(writer,&Message::FileChunk{id:id.clone(),data}).await?;
                        *deadline=Instant::now()+FILE_ACTIVITY;
                    }
                    None=>{
                        let id=id.clone();
                        let hash=source.digest();
                        protocol::send(writer,&Message::FileFinish{id:id.clone(),hash}).await?;
                        transfer_view(s,connection,&id,"finalizing");
                        transfer=Transfer::AwaitComplete{id,deadline:Instant::now()+FILE_ACTIVITY};
                    }
                }
            }
            action=actions.recv()=>{
                transfer.check_deadline()?;
                match action.ok_or(Error::Cancelled)? {
                    Action::SendFile{id,source}=>{
                        if !matches!(transfer,Transfer::Idle) {return Err(Error::Protocol);}
                        protocol::send(writer,&Message::FileOffer{id:id.clone(),name:source.name.clone(),size:source.size}).await?;
                        transfer_view(s,connection,&id,"awaiting_approval");
                        transfer=Transfer::AwaitAccept{id,source:*source,deadline:Instant::now()+FILE_APPROVAL};
                    }
                    Action::ReceiveFile{id,accept}=>{
                        let previous=std::mem::replace(&mut transfer,Transfer::Idle);
                        let Transfer::IncomingOffer{id:offered,name,size,..}=previous else {return Err(Error::Protocol);};
                        if id!=offered {return Err(Error::Protocol);}
                        if accept {
                            let sink=timeout(FILE_ACTIVITY,Sink::create(&s.receive_directory,&name,size)).await.map_err(|_|Error::Expired)??;
                            protocol::send(writer,&Message::FileAccept{id:id.clone(),accept:true}).await?;
                            transfer_view(s,connection,&id,"receiving");
                            transfer=Transfer::Receiving{id,sink,deadline:Instant::now()+FILE_ACTIVITY};
                        }else{
                            protocol::send(writer,&Message::FileAccept{id:id.clone(),accept:false}).await?;
                            transfer_view(s,connection,&id,"rejected");
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
                if !matches!(message,Message::FileChunk{..}) {
                    if control_window.elapsed()>=Duration::from_secs(60){control_window=Instant::now();controls=0;}
                    controls+=1;if controls>64{return Err(Error::Limit);}
                }
                match message {
                    Message::Ping{}=>protocol::send(writer,&Message::Pong{}).await?,
                    Message::Pong{}=>{},
                    Message::FileOffer{id,name,size}=>{
                        if !valid_id(&id){return Err(Error::Protocol);}
                        if seen_offers.contains(&id){return Err(Error::Protocol);}
                        if seen_offers.len()>=128{return Err(Error::Limit);}
                        seen_offers.insert(id.clone());
                        files::validate_name(&name)?;
                        if size>files::MAX_FILE_SIZE{return Err(Error::Limit);}
                        if offer_window.elapsed()>=Duration::from_secs(60){offer_window=Instant::now();offers=0;}
                        offers+=1;if offers>30{return Err(Error::Limit);}
                        if !matches!(transfer,Transfer::Idle)||!reserve_offer(s,connection,&id,&name,size)? {
                            protocol::send(writer,&Message::FileAccept{id,accept:false}).await?;
                        }else{
                            transfer=Transfer::IncomingOffer{id,name,size,deadline:Instant::now()+FILE_APPROVAL};
                        }
                    }
                    Message::FileAccept{id,accept}=>{
                        let previous=std::mem::replace(&mut transfer,Transfer::Idle);
                        let Transfer::AwaitAccept{id:offered,source,..}=previous else{return Err(Error::Protocol);};
                        if id!=offered{return Err(Error::Protocol);}
                        if accept {
                            transfer_view(s,connection,&id,"sending");
                            transfer=Transfer::Sending{id,source,deadline:Instant::now()+FILE_ACTIVITY};
                        }else{
                            transfer_view(s,connection,&id,"rejected");
                        }
                    }
                    Message::FileChunk{id,data}=>{
                        let Transfer::Receiving{id:expected,sink,deadline}=&mut transfer else{return Err(Error::Protocol);};
                        if &id!=expected{return Err(Error::Protocol);}
                        timeout(FILE_ACTIVITY,sink.write(&data)).await.map_err(|_|Error::Expired)??;
                        *deadline=Instant::now()+FILE_ACTIVITY;
                    }
                    Message::FileFinish{id,hash}=>{
                        let previous=std::mem::replace(&mut transfer,Transfer::Idle);
                        let Transfer::Receiving{id:expected,sink,..}=previous else{return Err(Error::Protocol);};
                        if id!=expected{return Err(Error::Protocol);}
                        if !store_lock(&store)?.is_trusted(peer){return Err(Error::Revoked);}
                        begin_finalization(s,connection,&id)?;
                        let shared=s.clone();let cid=connection.to_owned();let finished=id.clone();
                        // A verified filesystem commit may outlive cancellation.
                        // Its owner always publishes the actual completion/error.
                        let finalize=tokio::spawn(async move {
                            let result=sink.finish(&hash).await;
                            view(&shared,&cid,|entry|{
                                if let Some(transfer)=&mut entry.view.transfer && transfer.id==finished {
                                    match &result {
                                        Ok(path)=>{transfer.status="complete".into();transfer.path=Some(path.to_string_lossy().into_owned());transfer.error=None;}
                                        Err(error)=>{transfer.status="error".into();transfer.error=Some(error.to_string());}
                                    }
                                }
                            });
                            result
                        });
                        finalize.await.map_err(|_|Error::FileIo)??;
                        protocol::send(writer,&Message::FileComplete{id}).await?;
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
