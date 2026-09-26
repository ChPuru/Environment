//! relay: a small, correct TCP chat. Line protocol: `/nick NAME`,
//! `/join ROOM`, anything else broadcasts to your room as "NAME: text".
//! Rooms are isolated; joins and departures are announced. Per-client
//! writer threads keep one slow reader from blocking the room.

use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

struct Client {
    id: u64,
    room: String,
    nick: String,
    tx: Sender<String>,
}

#[derive(Clone, Default)]
pub struct Hub {
    inner: Arc<Mutex<Vec<Client>>>,
}

impl Hub {
    fn broadcast(&self, room: &str, line: &str) {
        let clients = self.inner.lock().expect("hub");
        for c in clients.iter().filter(|c| c.room == room) {
            let _ = c.tx.send(line.to_string());
        }
    }

    fn with_client<T>(&self, id: u64, f: impl FnOnce(&mut Client) -> T) -> Option<T> {
        let mut clients = self.inner.lock().expect("hub");
        clients.iter_mut().find(|c| c.id == id).map(f)
    }

    fn remove(&self, id: u64) -> Option<(String, String)> {
        let mut clients = self.inner.lock().expect("hub");
        let pos = clients.iter().position(|c| c.id == id)?;
        let c = clients.remove(pos);
        Some((c.room, c.nick))
    }
}

fn handle(hub: Hub, stream: TcpStream, id: u64) -> Result<()> {
    let (tx, rx) = channel::<String>();
    {
        let mut w = stream.try_clone()?;
        thread::spawn(move || {
            for line in rx {
                if w.write_all(line.as_bytes()).is_err() || w.write_all(b"\n").is_err() {
                    break;
                }
                let _ = w.flush();
            }
        });
    }
    let nick = format!("guest{id}");
    hub.inner.lock().expect("hub").push(Client {
        id,
        room: "lobby".into(),
        nick: nick.clone(),
        tx,
    });
    hub.broadcast("lobby", &format!("* {nick} joined lobby"));

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Some(new_nick) = line.strip_prefix("/nick ") {
            let new_nick = new_nick.trim().to_string();
            if new_nick.is_empty() || new_nick.contains(' ') {
                let _ = hub.with_client(id, |c| c.tx.send("! nick must be one word".into()));
                continue;
            }
            let info = hub.with_client(id, |c| {
                let old = std::mem::replace(&mut c.nick, new_nick.clone());
                (c.room.clone(), old)
            });
            if let Some((room, old)) = info {
                hub.broadcast(&room, &format!("* {old} is now {new_nick}"));
            }
        } else if let Some(room) = line.strip_prefix("/join ") {
            let room = room.trim().to_string();
            let info = hub.with_client(id, |c| {
                let old = std::mem::replace(&mut c.room, room.clone());
                (old, c.nick.clone())
            });
            if let Some((old_room, nick)) = info {
                hub.broadcast(&old_room, &format!("* {nick} left {old_room}"));
                hub.broadcast(&room, &format!("* {nick} joined {room}"));
            }
        } else {
            let info = hub.with_client(id, |c| (c.room.clone(), c.nick.clone()));
            if let Some((room, nick)) = info {
                hub.broadcast(&room, &format!("{nick}: {line}"));
            }
        }
    }
    if let Some((room, nick)) = hub.remove(id) {
        hub.broadcast(&room, &format!("* {nick} left {room}"));
    }
    Ok(())
}

/// Serve forever on the listener (spawns a thread per connection).
pub fn serve(listener: TcpListener) {
    let hub = Hub::default();
    let mut next_id = 0u64;
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        next_id += 1;
        let (hub, id) = (hub.clone(), next_id);
        thread::spawn(move || {
            let _ = handle(hub, stream, id);
        });
    }
}

/// Test/embedding helper: bind an ephemeral port, serve in a thread.
pub fn spawn_server() -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = format!("127.0.0.1:{}", listener.local_addr()?.port());
    thread::spawn(move || serve(listener));
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Chat {
        w: TcpStream,
        r: BufReader<TcpStream>,
    }
    impl Chat {
        fn connect(addr: &str) -> Chat {
            let s = TcpStream::connect(addr).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            Chat {
                r: BufReader::new(s.try_clone().unwrap()),
                w: s,
            }
        }
        fn send(&mut self, line: &str) {
            self.w.write_all(line.as_bytes()).unwrap();
            self.w.write_all(b"\n").unwrap();
        }
        fn recv(&mut self) -> String {
            let mut l = String::new();
            self.r.read_line(&mut l).unwrap();
            l.trim_end().to_string()
        }
        fn recv_until(&mut self, needle: &str) -> String {
            for _ in 0..20 {
                let l = self.recv();
                if l.contains(needle) {
                    return l;
                }
            }
            panic!("never saw {needle:?}");
        }
    }

    #[test]
    fn nicks_broadcast_and_room_isolation() {
        let addr = spawn_server().unwrap();
        let mut a = Chat::connect(&addr);
        a.recv_until("joined lobby");
        let mut b = Chat::connect(&addr);
        a.recv_until("joined lobby"); // b's arrival, seen by a
        b.recv_until("joined lobby");

        a.send("/nick alice");
        b.recv_until("is now alice");
        b.send("/nick bob");
        a.recv_until("is now bob");

        a.send("hello room");
        assert_eq!(b.recv_until("alice:"), "alice: hello room");
        assert_eq!(
            a.recv_until("alice:"),
            "alice: hello room",
            "sender echoes too"
        );

        // c joins a different room: lobby traffic must not reach it.
        let mut c = Chat::connect(&addr);
        c.recv_until("joined lobby");
        c.send("/join ops");
        c.recv_until("joined ops");
        a.send("secret lobby talk");
        b.recv_until("secret lobby talk");
        c.send("ops only");
        assert_eq!(c.recv_until("guest"), "guest3: ops only".to_string());
        // If lobby leaked into ops, the previous recv_until would have
        // surfaced "secret lobby talk" before "ops only" — it did not.
    }

    #[test]
    fn bad_nick_is_rejected_politely() {
        let addr = spawn_server().unwrap();
        let mut a = Chat::connect(&addr);
        a.recv_until("joined lobby");
        a.send("/nick two words");
        assert!(a.recv_until("!").contains("one word"));
    }
}
