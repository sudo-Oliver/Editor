//! The network side of a session. The host is the hub: guests connect to
//! it, and it passes every update on to the others. Each editor applies
//! updates to its own `Project` on its own thread, in step with its text;
//! the session keeps a copy only to answer newcomers and to check roles.
//! All of it runs on a small runtime that exists only while a session is open.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc as std_mpsc,
    },
    time::Duration,
};

use iroh::{
    Endpoint, EndpointId,
    address_lookup::UserData,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;
use tokio::sync::{mpsc, oneshot};

use crate::{
    model::Project,
    protocol::{ALPN, CommentIntent, Frame, Hello, MAX_FRAME, Presence, Role, Ticket, read_frame, same_secret, write_frame},
};

const SERVICE: &str = "editor-collab";
const HOST: u64 = 0;

/// How often a guest tries to get back to the host before giving up, about a
/// minute and a half in all.
const RECONNECT_ATTEMPTS: u32 = 10;
/// An update this large would be refused at the other end, which would drop
/// the link and try again forever. Room for the frame's own header.
const MAX_UPDATE: usize = MAX_FRAME - 1024;

#[derive(Clone, Debug)]
pub struct Identity {
    pub name: String,
    pub color: u32,
}

#[derive(Debug)]
pub enum Event {
    /// An update for the local `Project`.
    Update(Vec<u8>),
    /// A reader's comment, which the host makes on their behalf.
    Intent { name: String, color: u32, intent: CommentIntent },
    Presence(Presence),
    PeerLeft(u64),
    /// Someone on the local network wants in, without a code.
    JoinRequest { peer: u64, name: String },
    /// The host let us in, at first or again after the link broke.
    Joined(Role),
    /// The link to the host broke. Edits made meanwhile merge once it is back.
    Reconnecting,
    /// Something the host should know, such as a guest that went too far.
    Warning(String),
    Ended(String),
}

/// A session hosted on the local network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanSession {
    pub host: EndpointId,
    pub name: String,
}

enum Command {
    Update(Vec<u8>),
    Intent(CommentIntent),
    Presence { path: String, anchor: usize, head: usize },
    Admit { peer: u64, role: Option<Role> },
    Leave,
    /// Cuts every link, as a broken network would.
    #[cfg(test)]
    DropLinks,
}

struct Guest {
    endpoint: EndpointId,
    name: String,
    color: u32,
    role: Role,
}

enum Input {
    Command(Command),
    Connected { peer: u64, sender: mpsc::UnboundedSender<Frame>, guest: Option<Guest> },
    Frame { peer: u64, frame: Frame },
    Closed { peer: u64 },
    Pending { peer: u64, name: String, reply: oneshot::Sender<Option<Role>> },
    /// A guest got back to the host, under a new id.
    Reconnected { connection: Connection, me: u64, role: Role },
    GaveUp(String),
}

type Wake = Arc<dyn Fn() + Send + Sync>;

/// Who the host lets in, shared between the actor and the door.
#[derive(Default)]
struct Admission {
    known: HashMap<EndpointId, Role>,
    banned: HashSet<EndpointId>,
}

pub struct Session {
    inputs: mpsc::UnboundedSender<Input>,
    events: std_mpsc::Receiver<Event>,
    /// The invite codes of the host, one per role.
    tickets: Vec<(Role, String)>,
    /// Fires once the goodbye is out.
    finished: Option<oneshot::Receiver<()>>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Session {
    /// Shares a project, given as `Project::encode_state`. `lan_name` makes
    /// the session visible on the local network under that name.
    pub fn host(identity: Identity, state: &[u8], lan_name: Option<String>, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let runtime = runtime()?;
        let (inputs, inputs_rx) = mpsc::unbounded_channel();
        let (events_tx, events) = std_mpsc::channel();
        let wake: Wake = Arc::new(wake);
        let (finish, finished) = oneshot::channel();
        let replica = Project::from_state(1, state)?;
        let tickets = runtime.block_on(async {
            let (endpoint, mdns) = bind(lan_name.as_deref()).await?;
            let tickets: Vec<(Role, Ticket)> = Role::ALL.into_iter().map(|role| (role, Ticket::new(endpoint.id()))).collect();
            let admission = Arc::new(Mutex::new(Admission::default()));
            let router = Router::builder(endpoint)
                .accept(
                    ALPN,
                    Host {
                        secrets: tickets.iter().map(|(role, ticket)| (*role, ticket.secret)).collect(),
                        open_on_lan: lan_name.is_some(),
                        admission: admission.clone(),
                        next_peer: Arc::new(AtomicU64::new(1)),
                        inputs: inputs.clone(),
                    },
                )
                .spawn();
            let actor = Actor::new(replica, identity, HOST, Role::Editor, Link::Host(router, admission), mdns, events_tx, wake, inputs.clone());
            tokio::spawn(actor.run(inputs_rx, finish));
            Ok::<_, String>(tickets.into_iter().map(|(role, ticket)| (role, ticket.encode())).collect())
        })?;
        Ok(Self {
            inputs,
            events,
            tickets,
            finished: Some(finished),
            runtime: Some(runtime),
        })
    }

    /// Joins with an invitation code.
    pub fn join(code: &str, identity: Identity, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let ticket = Ticket::decode(code)?;
        Self::connect(ticket.host, Some(ticket.secret.to_vec()), identity, wake)
    }

    /// Asks the host of a local session to let us in.
    pub fn join_lan(host: EndpointId, identity: Identity, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        Self::connect(host, None, identity, wake)
    }

    /// Returns at once. `Event::Joined` or `Event::Ended` tells how it went,
    /// since the host may take a while, or a person has to let us in.
    fn connect(host: EndpointId, secret: Option<Vec<u8>>, identity: Identity, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let runtime = runtime()?;
        let (inputs, inputs_rx) = mpsc::unbounded_channel();
        let (events_tx, events) = std_mpsc::channel();
        let wake: Wake = Arc::new(wake);
        let (finish, finished) = oneshot::channel();
        let links = inputs.clone();
        runtime.spawn(async move {
            let joined = async {
                let (endpoint, mdns) = bind(None).await?;
                let (connection, send, recv, me, role) = handshake(&endpoint, host, secret.clone(), &identity).await?;
                Ok::<_, String>((endpoint, mdns, connection, send, recv, me, role))
            };
            match joined.await {
                Ok((endpoint, mdns, connection, send, recv, me, role)) => {
                    events_tx.send(Event::Joined(role)).ok();
                    let link = Link::Guest {
                        endpoint,
                        host,
                        secret,
                        connection: Some(connection),
                    };
                    let actor = Actor::new(Project::new(2), identity, me, role, link, mdns, events_tx, wake, links.clone());
                    link_streams(HOST, send, recv, None, links);
                    actor.run(inputs_rx, finish).await;
                }
                Err(reason) => {
                    events_tx.send(Event::Ended(reason)).ok();
                    wake();
                }
            }
        });
        Ok(Self {
            inputs,
            events,
            tickets: Vec::new(),
            finished: Some(finished),
            runtime: Some(runtime),
        })
    }

    /// The invitation code for `role`, for the host.
    pub fn ticket(&self, role: Role) -> Option<&str> {
        self.tickets.iter().find(|(each, _)| *each == role).map(|(_, code)| code.as_str())
    }

    pub fn is_host(&self) -> bool {
        !self.tickets.is_empty()
    }

    /// Sends an update of the local `Project` to the others.
    pub fn send_update(&self, update: Vec<u8>) {
        if !update.is_empty() {
            self.command(Command::Update(update));
        }
    }

    /// Asks the host to comment on our behalf, for readers.
    pub fn send_intent(&self, intent: CommentIntent) {
        self.command(Command::Intent(intent));
    }

    pub fn presence(&self, path: &str, anchor: usize, head: usize) {
        self.command(Command::Presence {
            path: path.to_string(),
            anchor,
            head,
        });
    }

    /// Lets someone from the local network in with `role`, or not.
    pub fn admit(&self, peer: u64, role: Option<Role>) {
        self.command(Command::Admit { peer, role });
    }

    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    fn command(&self, command: Command) {
        self.inputs.send(Input::Command(command)).ok();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.command(Command::Leave);
        if let Some(runtime) = self.runtime.take() {
            // Shutting down drops every task at once, so the goodbye goes
            // first, without blocking the UI for long.
            if let Some(finished) = self.finished.take() {
                runtime.block_on(async {
                    tokio::time::timeout(Duration::from_millis(500), finished).await.ok();
                });
            }
            runtime.shutdown_background();
        }
    }
}

/// Watches the local network for hosted sessions while it lives.
pub struct LanBrowser {
    sessions: Arc<Mutex<Vec<LanSession>>>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl LanBrowser {
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let runtime = runtime()?;
        let sessions = Arc::new(Mutex::new(Vec::<LanSession>::new()));
        let found = sessions.clone();
        runtime.block_on(async {
            let (endpoint, mdns) = bind(None).await?;
            tokio::spawn(async move {
                let _endpoint = endpoint;
                let mut events = mdns.subscribe().await;
                while let Some(event) = events.next().await {
                    let mut sessions = found.lock().unwrap();
                    match event {
                        DiscoveryEvent::Discovered { endpoint_info, .. } => {
                            let Some(name) = endpoint_info.data.user_data() else {
                                continue;
                            };
                            sessions.retain(|session| session.host != endpoint_info.endpoint_id);
                            sessions.push(LanSession {
                                host: endpoint_info.endpoint_id,
                                name: name.to_string(),
                            });
                        }
                        DiscoveryEvent::Expired { endpoint_id } => sessions.retain(|session| session.host != endpoint_id),
                        _ => continue,
                    }
                    drop(sessions);
                    wake();
                }
            });
            Ok::<_, String>(())
        })?;
        Ok(Self {
            sessions,
            runtime: Some(runtime),
        })
    }

    pub fn sessions(&self) -> Vec<LanSession> {
        self.sessions.lock().unwrap().clone()
    }
}

impl Drop for LanBrowser {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("collab")
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
}

async fn bind(lan_name: Option<&str>) -> Result<(Endpoint, MdnsAddressLookup), String> {
    let mut builder = Endpoint::builder(presets::N0);
    if let Some(name) = lan_name {
        let name: String = name.chars().take(60).collect();
        builder = builder.user_data_for_address_lookup(UserData::try_from(name).map_err(|error| error.to_string())?);
    }
    let endpoint = builder.bind().await.map_err(|error| error.to_string())?;
    let mdns = MdnsAddressLookup::builder()
        .advertise(lan_name.is_some())
        .service_name(SERVICE)
        .build(endpoint.id())
        .map_err(|error| error.to_string())?;
    endpoint.address_lookup().map_err(|error| error.to_string())?.add(mdns.clone());
    Ok((endpoint, mdns))
}

/// Connects to the host and asks to come in. Returns the id and the role
/// the host gave us.
async fn handshake(
    endpoint: &Endpoint,
    host: EndpointId,
    secret: Option<Vec<u8>>,
    identity: &Identity,
) -> Result<(Connection, SendStream, RecvStream, u64, Role), String> {
    let connection = endpoint.connect(host, ALPN).await.map_err(|error| format!("Cannot reach the host: {error}"))?;
    let (mut send, mut recv) = connection.open_bi().await.map_err(|error| error.to_string())?;
    let hello = Hello {
        secret,
        name: identity.name.clone(),
        color: identity.color,
    };
    write_frame(&mut send, &Frame::Hello(hello)).await?;
    match read_frame(&mut recv).await? {
        Frame::Welcome(me, role) => Ok((connection, send, recv, me, role)),
        Frame::Denied => Err("The host declined".to_string()),
        _ => Err("The host sent something unexpected".to_string()),
    }
}

/// Starts reading and writing one connection, reporting to the actor.
fn link_streams(peer: u64, mut send: SendStream, mut recv: RecvStream, guest: Option<Guest>, inputs: mpsc::UnboundedSender<Input>) {
    let (sender, mut outgoing) = mpsc::unbounded_channel::<Frame>();
    inputs.send(Input::Connected { peer, sender, guest }).ok();
    tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            if write_frame(&mut send, &frame).await.is_err() {
                break;
            }
        }
        send.finish().ok();
    });
    tokio::spawn(async move {
        while let Ok(frame) = read_frame(&mut recv).await {
            if inputs.send(Input::Frame { peer, frame }).is_err() {
                return;
            }
        }
        inputs.send(Input::Closed { peer }).ok();
    });
}

/// Lets guests in, for the host.
#[derive(Clone)]
struct Host {
    secrets: Vec<(Role, [u8; 16])>,
    open_on_lan: bool,
    admission: Arc<Mutex<Admission>>,
    next_peer: Arc<AtomicU64>,
    inputs: mpsc::UnboundedSender<Input>,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Host")
    }
}

impl ProtocolHandler for Host {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let Ok(Frame::Hello(hello)) = read_frame(&mut recv).await else {
            return Ok(());
        };
        let remote = connection.remote_id();
        let peer = self.next_peer.fetch_add(1, Ordering::Relaxed);
        let (banned, known) = {
            let admission = self.admission.lock().unwrap();
            (admission.banned.contains(&remote), admission.known.get(&remote).copied())
        };
        let role = match &hello.secret {
            _ if banned => None,
            // Guests let in once come back without asking, after their link broke.
            _ if known.is_some() => known,
            Some(secret) => self.secrets.iter().find(|(_, own)| same_secret(secret, own)).map(|(role, _)| *role),
            None if self.open_on_lan => {
                let (reply, answer) = oneshot::channel();
                self.inputs
                    .send(Input::Pending {
                        peer,
                        name: hello.name.clone(),
                        reply,
                    })
                    .ok();
                answer.await.ok().flatten()
            }
            None => None,
        };
        let Some(role) = role else {
            write_frame(&mut send, &Frame::Denied).await.ok();
            send.finish().ok();
            connection.closed().await;
            return Ok(());
        };
        self.admission.lock().unwrap().known.insert(remote, role);
        if write_frame(&mut send, &Frame::Welcome(peer, role)).await.is_err() {
            return Ok(());
        }
        let guest = Guest {
            endpoint: remote,
            name: hello.name.chars().take(40).collect(),
            color: hello.color,
            role,
        };
        link_streams(peer, send, recv, Some(guest), self.inputs.clone());
        connection.closed().await;
        Ok(())
    }
}

/// How this editor is connected.
enum Link {
    Host(Router, Arc<Mutex<Admission>>),
    Guest {
        endpoint: Endpoint,
        host: EndpointId,
        secret: Option<Vec<u8>>,
        /// Missing while reconnecting.
        connection: Option<Connection>,
    },
}

struct Peer {
    sender: mpsc::UnboundedSender<Frame>,
    presence: Option<Presence>,
    /// Missing for the host, as seen by a guest.
    guest: Option<Guest>,
}

/// Owns the copy of the project and every link, and is the only one to
/// touch them.
struct Actor {
    replica: Project,
    identity: Identity,
    me: u64,
    role: Role,
    link: Link,
    _mdns: MdnsAddressLookup,
    peers: HashMap<u64, Peer>,
    pending: HashMap<u64, oneshot::Sender<Option<Role>>>,
    events: std_mpsc::Sender<Event>,
    wake: Wake,
    inputs: mpsc::UnboundedSender<Input>,
}

impl Actor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        replica: Project,
        identity: Identity,
        me: u64,
        role: Role,
        link: Link,
        mdns: MdnsAddressLookup,
        events: std_mpsc::Sender<Event>,
        wake: Wake,
        inputs: mpsc::UnboundedSender<Input>,
    ) -> Self {
        Self {
            replica,
            identity,
            me,
            role,
            link,
            _mdns: mdns,
            peers: HashMap::new(),
            pending: HashMap::new(),
            events,
            wake,
            inputs,
        }
    }

    fn is_host(&self) -> bool {
        self.me == HOST
    }

    async fn run(mut self, mut inputs: mpsc::UnboundedReceiver<Input>, finish: oneshot::Sender<()>) {
        while let Some(input) = inputs.recv().await {
            let mut go_on = self.handle(input);
            // Inputs come in bursts, and one wake covers all of them.
            while go_on && let Ok(input) = inputs.try_recv() {
                go_on = self.handle(input);
            }
            (self.wake)();
            if !go_on {
                break;
            }
        }
        for peer in self.peers.values() {
            peer.sender.send(Frame::Left(self.me)).ok();
        }
        // The writers need a moment to send the goodbye.
        tokio::time::sleep(Duration::from_millis(100)).await;
        finish.send(()).ok();
        match self.link {
            Link::Host(router, _) => {
                router.shutdown().await.ok();
            }
            Link::Guest { endpoint, connection, .. } => {
                if let Some(connection) = connection {
                    connection.close(0u32.into(), b"bye");
                }
                endpoint.close().await;
            }
        }
    }

    /// Returns false once the session is over.
    fn handle(&mut self, input: Input) -> bool {
        match input {
            Input::Command(command) => return self.command(command),
            Input::Pending { peer, name, reply } => {
                self.pending.insert(peer, reply);
                self.emit(Event::JoinRequest { peer, name });
            }
            Input::Connected { peer, sender, guest } => {
                sender.send(Frame::Sync(self.replica.state_vector())).ok();
                // A newcomer sees where everyone already is.
                for other in self.peers.values() {
                    if let Some(presence) = &other.presence {
                        sender.send(Frame::Presence(presence.clone())).ok();
                    }
                }
                self.peers.insert(peer, Peer { sender, presence: None, guest });
            }
            Input::Closed { peer } => {
                self.peers.remove(&peer);
                if !self.is_host() && peer == HOST {
                    self.reconnect();
                    return true;
                }
                self.broadcast(None, || Frame::Left(peer));
                self.emit(Event::PeerLeft(peer));
            }
            Input::Reconnected { connection, me, role } => {
                self.me = me;
                self.role = role;
                if let Link::Guest { connection: current, .. } = &mut self.link {
                    *current = Some(connection);
                }
                self.emit(Event::Joined(role));
            }
            Input::GaveUp(reason) => {
                self.emit(Event::Ended(reason));
                return false;
            }
            Input::Frame { peer, frame } => return self.receive(peer, frame),
        }
        true
    }

    fn command(&mut self, command: Command) -> bool {
        match command {
            Command::Update(update) => {
                if update.len() > MAX_UPDATE {
                    self.emit(Event::Warning("This change is too large to share live. Others get it once it is smaller, or as a file.".to_string()));
                } else if self.replica.apply(&update).is_ok() {
                    self.replica.forget_events();
                    self.broadcast(None, || Frame::Update(update.clone()));
                }
            }
            Command::Intent(intent) => self.broadcast(None, || Frame::Intent(intent.clone())),
            Command::Presence { path, anchor, head } => {
                let presence = Presence {
                    peer: self.me,
                    name: self.identity.name.clone(),
                    color: self.identity.color,
                    role: self.role,
                    path,
                    anchor,
                    head,
                };
                self.broadcast(None, || Frame::Presence(presence.clone()));
            }
            Command::Admit { peer, role } => {
                if let Some(reply) = self.pending.remove(&peer) {
                    reply.send(role).ok();
                }
            }
            Command::Leave => return false,
            #[cfg(test)]
            Command::DropLinks => self.peers.clear(),
        }
        true
    }

    /// Tries to get back to the host in the background. The project stays,
    /// so whatever happens meanwhile merges once the link is back.
    fn reconnect(&mut self) {
        let Link::Guest {
            endpoint,
            host,
            secret,
            connection,
        } = &mut self.link
        else {
            return;
        };
        *connection = None;
        let (endpoint, host, secret, identity, inputs) = (endpoint.clone(), *host, secret.clone(), self.identity.clone(), self.inputs.clone());
        self.emit(Event::Reconnecting);
        tokio::spawn(async move {
            let mut delay = Duration::from_secs(1);
            let mut reason = String::new();
            for _ in 0..RECONNECT_ATTEMPTS {
                tokio::time::sleep(delay).await;
                match handshake(&endpoint, host, secret.clone(), &identity).await {
                    Ok((connection, send, recv, me, role)) => {
                        inputs.send(Input::Reconnected { connection, me, role }).ok();
                        link_streams(HOST, send, recv, None, inputs);
                        return;
                    }
                    Err(error) if error.contains("declined") => {
                        reason = error;
                        break;
                    }
                    Err(error) => reason = error,
                }
                delay = (delay * 2).min(Duration::from_secs(15));
            }
            inputs.send(Input::GaveUp(format!("Lost the host: {reason}"))).ok();
        });
    }

    /// Returns false once the session is over.
    fn receive(&mut self, from: u64, frame: Frame) -> bool {
        let role = self.peers.get(&from).and_then(|peer| peer.guest.as_ref()).map(|guest| guest.role);
        match frame {
            Frame::Sync(state_vector) => {
                if let Ok(update) = self.replica.diff(&state_vector)
                    && let Some(peer) = self.peers.get(&from)
                {
                    if update.len() > MAX_UPDATE {
                        let name = peer.guest.as_ref().map_or_else(|| "the host".to_string(), |guest| guest.name.clone());
                        self.emit(Event::Warning(format!("The project is too large to send to {name}. Leave out large files and share again.")));
                    } else {
                        peer.sender.send(Frame::Update(update)).ok();
                    }
                }
            }
            Frame::Update(update) => match role {
                // Readers comment through the host, and nothing they send counts.
                Some(Role::Reader) => {}
                Some(Role::Reviewer) => {
                    let checked = self.replica.apply_suggestions_only(&update);
                    self.accept_update(from, update, checked.is_ok());
                    if let Err(reason) = checked {
                        self.ban(from, &reason);
                    }
                }
                _ => {
                    let applied = self.replica.apply(&update).is_ok();
                    self.accept_update(from, update, applied);
                }
            },
            Frame::Intent(intent) => {
                if self.is_host()
                    && let Some(guest) = self.peers.get(&from).and_then(|peer| peer.guest.as_ref())
                {
                    self.emit(Event::Intent {
                        name: guest.name.clone(),
                        color: guest.color,
                        intent,
                    });
                }
            }
            Frame::Presence(mut presence) => {
                if self.is_host() {
                    // Guests cannot speak for anyone else.
                    let Some(peer) = self.peers.get_mut(&from) else {
                        return true;
                    };
                    presence.peer = from;
                    if let Some(guest) = &peer.guest {
                        presence.name = guest.name.clone();
                        presence.color = guest.color;
                        presence.role = guest.role;
                    }
                    peer.presence = Some(presence.clone());
                    self.broadcast(Some(from), || Frame::Presence(presence.clone()));
                }
                self.emit(Event::Presence(presence));
            }
            Frame::Left(HOST) if !self.is_host() => {
                self.emit(Event::Ended("The host ended the session".to_string()));
                return false;
            }
            Frame::Left(peer) => self.emit(Event::PeerLeft(peer)),
            Frame::Hello(_) | Frame::Welcome(..) | Frame::Denied => {}
        }
        true
    }

    /// Hands an applied update to the local project, and on to the others.
    /// Even a rejected one stays in, since a CRDT cannot take it back, so
    /// everyone keeps the same text.
    fn accept_update(&mut self, from: u64, update: Vec<u8>, applied: bool) {
        if !applied {
            return;
        }
        self.replica.forget_events();
        if self.is_host() {
            self.broadcast(Some(from), || Frame::Update(update.clone()));
        }
        self.emit(Event::Update(update));
    }

    /// Sends a guest away for good, for this session.
    fn ban(&mut self, peer: u64, reason: &str) {
        let Some(removed) = self.peers.remove(&peer) else {
            return;
        };
        let name = removed.guest.as_ref().map_or_else(String::new, |guest| guest.name.clone());
        if let (Link::Host(_, admission), Some(guest)) = (&self.link, &removed.guest) {
            let mut admission = admission.lock().unwrap();
            admission.known.remove(&guest.endpoint);
            admission.banned.insert(guest.endpoint);
        }
        self.broadcast(None, || Frame::Left(peer));
        self.emit(Event::PeerLeft(peer));
        self.emit(Event::Warning(format!("{name} was removed from the session: {reason}.")));
    }

    fn broadcast(&self, except: Option<u64>, frame: impl Fn() -> Frame) {
        for (id, peer) in &self.peers {
            if Some(*id) != except {
                peer.sender.send(frame()).ok();
            }
        }
    }

    fn emit(&self, event: Event) {
        self.events.send(event).ok();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use editor_core::TextChange;

    use super::*;
    use crate::{model::Author, track::track};

    thread_local! {
        /// Events polled while waiting for an earlier one.
        static LEFT_OVER: std::cell::RefCell<Vec<(usize, Event)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Waits for an event, keeping the project of `session` up to date.
    #[track_caller]
    fn wait_for(session: &Session, project: &mut Project, mut done: impl FnMut(&Event) -> bool) -> Event {
        let key = session as *const Session as usize;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let polled = session.poll();
            for event in &polled {
                if let Event::Update(update) = event {
                    project.apply(update).unwrap();
                }
            }
            let found = LEFT_OVER.with_borrow_mut(|left_over| {
                left_over.extend(polled.into_iter().map(|event| (key, event)));
                let index = left_over.iter().position(|(owner, event)| *owner == key && done(event))?;
                let (_, event) = left_over.remove(index);
                let mut position = 0;
                left_over.retain(|(owner, _)| {
                    position += 1;
                    *owner != key || position > index
                });
                Some(event)
            });
            if let Some(event) = found {
                return event;
            }
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Keeps `project` up to date until `done` holds.
    #[track_caller]
    fn sync_until(session: &Session, project: &mut Project, mut done: impl FnMut(&mut Project) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done(project) {
            assert!(Instant::now() < deadline, "timed out");
            for event in session.poll() {
                match event {
                    Event::Update(update) => project.apply(&update).unwrap(),
                    other => LEFT_OVER.with_borrow_mut(|left_over| left_over.push((session as *const Session as usize, other))),
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn identity(name: &str) -> Identity {
        Identity {
            name: name.to_string(),
            color: 0x3366ff,
        }
    }

    /// Three editors and the host type at once, as fast as they can, over the
    /// real network stack: everyone must end with the same text, quickly.
    /// cargo test -p collab --release -- --ignored many_editors --nocapture
    #[test]
    #[ignore]
    fn many_editors_converge_under_load() {
        const EDITS: usize = 300;
        let start = "\\section{Intro}\nSome text to edit, with Ümlauts.\n".repeat(50);
        let mut host_project = Project::new(20);
        host_project.add_file("main.tex", &start);
        let host = Session::host(identity("Host"), &host_project.encode_state(), None, || {}).unwrap();
        let code = host.ticket(Role::Editor).unwrap().to_string();
        let mut guests: Vec<(Session, Project)> = (0..3)
            .map(|index| {
                let session = Session::join(&code, identity(&format!("Guest {index}")), || {}).unwrap();
                let mut project = Project::new(21 + index as u64);
                wait_for(&session, &mut project, |event| matches!(event, Event::Joined(_)));
                sync_until(&session, &mut project, |project| project.content("main.tex") == start);
                (session, project)
            })
            .collect();

        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut random = move |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below.max(1) as u64) as usize
        };
        let started = Instant::now();
        for _ in 0..EDITS {
            for index in 0..=guests.len() {
                let (session, project) = match index {
                    0 => (&host, &mut host_project),
                    _ => {
                        let (session, project) = &mut guests[index - 1];
                        (&*session, project)
                    }
                };
                for event in session.poll() {
                    if let Event::Update(update) = event {
                        project.apply(&update).unwrap();
                    }
                }
                project.take_remote();
                let text = project.content("main.tex");
                let mut at = random(text.len() + 1);
                while !text.is_char_boundary(at) {
                    at -= 1;
                }
                let mut end = (at + random(3)).min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                let change = if random(3) == 0 { TextChange::new(at, end - at, "") } else { TextChange::new(at, 0, ["a", "ß", "\n", "{}"][random(4)]) };
                session.send_update(project.edit("main.tex", &[change]));
            }
        }
        let typed = started.elapsed();
        let expected = loop {
            for (session, project) in &mut guests {
                for event in session.poll() {
                    if let Event::Update(update) = event {
                        project.apply(&update).unwrap();
                    }
                }
            }
            for event in host.poll() {
                if let Event::Update(update) = event {
                    host_project.apply(&update).unwrap();
                }
            }
            let text = host_project.content("main.tex");
            if guests.iter_mut().all(|(_, project)| project.content("main.tex") == text) {
                break text;
            }
            assert!(started.elapsed() < Duration::from_secs(60), "editors did not converge");
            std::thread::sleep(Duration::from_millis(5));
        };
        println!("{} edits typed in {typed:?}, all equal after {:?}, {} bytes", EDITS * 4, started.elapsed(), expected.len());

        // One more edit, timed from typing to arriving everywhere.
        let sent = Instant::now();
        host.send_update(host_project.edit("main.tex", &[TextChange::new(0, 0, "#")]));
        for (session, project) in &mut guests {
            sync_until(session, project, |project| project.content("main.tex").starts_with('#'));
        }
        println!("one edit reached every guest in {:?}", sent.elapsed());
        assert!(sent.elapsed() < Duration::from_secs(2));
    }

    /// Runs over the real network stack, so it needs a network.
    #[test]
    #[ignore]
    fn roles_and_edits_flow_over_the_network() {
        let mut host_project = Project::new(10);
        host_project.add_file("main.tex", "Hallo");
        let host = Session::host(identity("Ada"), &host_project.encode_state(), Some("Paper".into()), || {}).unwrap();
        let code = |role| host.ticket(role).unwrap().to_string();

        let mut grace_project = Project::new(11);
        let grace = Session::join(&code(Role::Editor), identity("Grace"), || {}).unwrap();
        assert!(matches!(wait_for(&grace, &mut grace_project, |event| matches!(event, Event::Joined(_))), Event::Joined(Role::Editor)));
        sync_until(&grace, &mut grace_project, |project| project.content("main.tex") == "Hallo");
        grace.send_update(grace_project.edit("main.tex", &[TextChange::new(5, 0, " Welt")]));
        sync_until(&host, &mut host_project, |project| project.content("main.tex") == "Hallo Welt");
        grace.presence("main.tex", 2, 4);
        let Event::Presence(presence) = wait_for(&host, &mut host_project, |event| matches!(event, Event::Presence(_))) else {
            unreachable!()
        };
        assert_eq!((presence.name.as_str(), presence.role, presence.head), ("Grace", Role::Editor, 4));

        // A reader's edits never count, but their comments come through the host.
        let mut linus_project = Project::new(12);
        let linus = Session::join(&code(Role::Reader), identity("Linus"), || {}).unwrap();
        assert!(matches!(wait_for(&linus, &mut linus_project, |event| matches!(event, Event::Joined(_))), Event::Joined(Role::Reader)));
        linus.send_update(Project::new(13).add_file("evil.tex", "x"));
        linus.send_intent(CommentIntent::Add {
            path: "main.tex".into(),
            range: 0..5,
            text: "Nice".into(),
            parent: None,
        });
        let Event::Intent { name, .. } = wait_for(&host, &mut host_project, |event| matches!(event, Event::Intent { .. })) else {
            unreachable!()
        };
        assert_eq!(name, "Linus");
        assert_eq!(host_project.paths(), ["main.tex"]);

        // A reviewer who edits outside suggestions is sent away.
        let mut eve_project = Project::new(14);
        let eve = Session::join(&code(Role::Reviewer), identity("Eve"), || {}).unwrap();
        wait_for(&eve, &mut eve_project, |event| matches!(event, Event::Joined(Role::Reviewer)));
        sync_until(&eve, &mut eve_project, |project| !project.content("main.tex").is_empty());
        let author = Author {
            name: "Eve".into(),
            color: 1,
        };
        let suggestion = track(&[TextChange::new(0, 0, "> ")], &[]);
        eve.send_update(eve_project.edit_tracked("main.tex", &suggestion.ops, &author));
        sync_until(&host, &mut host_project, |project| project.suggestions().len() == 1);
        eve.send_update(eve_project.edit("main.tex", &[TextChange::new(2, 2, "")]));
        wait_for(&host, &mut host_project, |event| matches!(event, Event::Warning(_)));

        // A broken link heals, and what the guest typed meanwhile arrives.
        host.command(Command::DropLinks);
        wait_for(&grace, &mut grace_project, |event| matches!(event, Event::Reconnecting));
        grace.send_update(grace_project.edit("main.tex", &[TextChange::new(0, 0, "! ")]));
        wait_for(&grace, &mut grace_project, |event| matches!(event, Event::Joined(_)));
        sync_until(&host, &mut host_project, |project| project.content("main.tex").starts_with("! "));

        drop(host);
        wait_for(&grace, &mut grace_project, |event| matches!(event, Event::Ended(_)));
    }
}
