//! Test utilities: muppets wired to one in-process fake y-sweet through
//! in-memory transports, with a fake clock and sequential client IDs.

use super::{Mode, Muppet};
use crate::ports::{Clock, IdSource, Outcome, Screen, Transport, View};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use xtxp_wire::fake_host::{ConnId, FakeHost};

#[derive(Default)]
struct Link {
    conn: Option<ConnId>,
    want_connect: bool,
    want_close: bool,
    outbox: Vec<Vec<u8>>,
    attempts: u32,
}

type Links = Rc<RefCell<Vec<Link>>>;

pub struct MemTransport {
    idx: usize,
    links: Links,
}

impl Transport for MemTransport {
    fn connect(&mut self) {
        let mut links = self.links.borrow_mut();
        links[self.idx].want_connect = true;
        links[self.idx].attempts += 1;
    }
    fn send(&mut self, frame: Vec<u8>) {
        self.links.borrow_mut()[self.idx].outbox.push(frame);
    }
    fn close(&mut self) {
        self.links.borrow_mut()[self.idx].want_close = true;
    }
}

#[derive(Clone)]
pub struct FakeClock(Rc<Cell<u64>>);

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}

pub struct SeqIds(u64);

impl IdSource for SeqIds {
    fn next_client_id(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

#[derive(Default)]
pub struct Recorded {
    pub shows: u32,
    pub last: Option<Screen>,
    pub statuses: Vec<String>,
    pub outcome: Option<Outcome>,
}

#[derive(Clone)]
pub struct RecordingView(pub Rc<RefCell<Recorded>>);

impl View for RecordingView {
    fn show(&mut self, screen: &Screen) {
        let mut r = self.0.borrow_mut();
        r.shows += 1;
        r.last = Some(screen.clone());
    }
    fn status(&mut self, line: &str) {
        self.0.borrow_mut().statuses.push(line.to_string());
    }
    fn finished(&mut self, outcome: &Outcome) {
        self.0.borrow_mut().outcome = Some(outcome.clone());
    }
}

pub type TestMuppet = Muppet<MemTransport, FakeClock, SeqIds, RecordingView>;

pub struct World {
    host: FakeHost,
    clock: Rc<Cell<u64>>,
    links: Links,
    muppets: Vec<TestMuppet>,
    views: Vec<RecordingView>,
    refuse: bool,
}

impl World {
    pub fn new() -> World {
        World {
            host: FakeHost::new(),
            clock: Rc::new(Cell::new(0)),
            links: Rc::new(RefCell::new(Vec::new())),
            muppets: Vec::new(),
            views: Vec::new(),
            refuse: false,
        }
    }

    pub fn add(&mut self, mode: Mode) -> usize {
        let idx = self.muppets.len();
        self.links.borrow_mut().push(Link::default());
        let view = RecordingView(Rc::new(RefCell::new(Recorded::default())));
        let ids = SeqIds(1000 * (idx as u64 + 1));
        let m = Muppet::new(
            mode,
            MemTransport {
                idx,
                links: self.links.clone(),
            },
            FakeClock(self.clock.clone()),
            ids,
            view.clone(),
        );
        self.muppets.push(m);
        self.views.push(view);
        idx
    }

    pub fn muppet(&self, i: usize) -> &TestMuppet {
        &self.muppets[i]
    }

    pub fn view(&self, i: usize) -> std::cell::Ref<'_, Recorded> {
        self.views[i].0.borrow()
    }

    pub fn shown(&self, i: usize) -> u32 {
        self.view(i).shows
    }

    pub fn outcome(&self, i: usize) -> Option<Outcome> {
        self.view(i).outcome.clone()
    }

    pub fn host_hash(&self) -> String {
        self.host.doc_hash()
    }

    pub fn refuse_connections(&mut self, refuse: bool) {
        self.refuse = refuse;
    }

    pub fn is_connected(&self, i: usize) -> bool {
        self.links.borrow()[i].conn.is_some()
    }

    pub fn connect_attempts(&self, i: usize) -> u32 {
        self.links.borrow()[i].attempts
    }

    /// The connection drops without either side choosing to close it.
    pub fn drop_connection(&mut self, i: usize) {
        let conn = self.links.borrow_mut()[i].conn.take();
        if let Some(conn) = conn {
            self.host.disconnect(conn);
        }
        self.muppets[i].on_disconnected();
    }

    /// Moves time forward in 10 ms steps, delivering everything at each step.
    pub fn advance(&mut self, ms: u64) {
        let mut left = ms;
        while left > 0 {
            let step = left.min(10);
            self.clock.set(self.clock.get() + step);
            left -= step;
            self.pump();
        }
    }

    pub fn run_until_finished(&mut self, i: usize) {
        self.pump();
        for _ in 0..3000 {
            if self.muppets[i].is_finished() {
                return;
            }
            self.advance(10);
        }
        panic!("muppet {i} didn't finish within 30 s of fake time");
    }

    /// Delivers connects, closes and frames until nothing moves; ticks every muppet.
    pub fn pump(&mut self) {
        for i in 0..self.muppets.len() {
            self.muppets[i].on_tick();
        }
        loop {
            let mut moved = false;
            for i in 0..self.muppets.len() {
                let (close, connect, outbox, conn) = {
                    let mut links = self.links.borrow_mut();
                    let l = &mut links[i];
                    let r = (
                        l.want_close,
                        l.want_connect,
                        std::mem::take(&mut l.outbox),
                        l.conn,
                    );
                    l.want_close = false;
                    l.want_connect = false;
                    r
                };
                if close {
                    moved = true;
                    if let Some(conn) = self.links.borrow_mut()[i].conn.take() {
                        self.host.disconnect(conn);
                    }
                    self.muppets[i].on_disconnected();
                    continue;
                }
                if connect {
                    moved = true;
                    if self.refuse {
                        self.muppets[i].on_disconnected();
                    } else {
                        let (conn, frames) = self.host.connect();
                        self.links.borrow_mut()[i].conn = Some(conn);
                        self.muppets[i].on_connected();
                        for f in frames {
                            self.muppets[i].on_frame(f);
                        }
                    }
                }
                if let Some(conn) = conn {
                    for frame in outbox {
                        moved = true;
                        for (to, f) in self.host.receive(conn, &frame) {
                            let j = self.links.borrow().iter().position(|l| l.conn == Some(to));
                            if let Some(j) = j {
                                self.muppets[j].on_frame(f);
                            }
                        }
                    }
                }
            }
            if !moved {
                break;
            }
        }
    }
}
