// `ChatServer`はアクターです。接続クライアントセッションのリストを維持します。
// そして、利用可能な部屋を管理します。ピアは、 `ChatServer`を介して同じ部屋の他のピアにメッセージを送信します。

use actix::prelude::*;
use rand::{ self, rngs::ThreadRng, Rng };

use std::sync::{
    atomic::{ AtomicUsize, Ordering},
    Arc,
};

use std::collections::{ HashMap, HashSet};

// チャットサーバーはこのメッセージをセッションに送信します
#[derive(Message)]
#[rtype(result="()")]
pub struct Message(pub String);

// チャットサーバー通信のメッセージ

// 新しいチャットセッションが作成されます
#[derive(Message)]
#[rtype(usize)]
pub struct Connect {
    pub addr: Recipient<Message>,
}

/// Session is disconnected
#[derive(Message)]
#[rtype(result = "()")]
pub struct Disconnect {
    pub id: usize,
}

// 特定の部屋にメッセージを送信する
#[derive(Message)]
#[rtype(result = "()")]
pub struct ClientMessage {
    /// Id of the client session
    pub id: usize,
    /// Peer message
    pub msg: String,
    /// Room name
    pub room: String,
}

// 利用可能な部屋のリスト
pub struct ListRooms;

impl actix::Message for ListRooms {
    type Result = Vec<String>;
}

// 部屋に参加します。部屋が存在しない場合は、新しい部屋を作成します。
#[derive(Message)]
#[rtype(result="()")]
pub struct Join {
    // Client Id
    pub id: usize,
    // Room name
    pub name: String,
}

// `ChatServer`はチャットルームを管理し、チャットセッションの調整を担当します。実装は超原始的です
pub struct ChatServer {
    sessions: HashMap<usize, Recipient<Message>>,
    rooms: HashMap<String, HashSet<usize>>,
    rng: ThreadRng,
    visitor_count: Arc<AtomicUsize>,
}

impl ChatServer {
    pub fn new(visitor_count: Arc<AtomicUsize>)-> ChatServer {
        // default room
        let mut rooms = HashMap::new();
        rooms.insert("Main".to_owned(), HashSet::new());

        ChatServer {
            sessions: HashMap::new(),
            rooms,
            rng: rand::thread_rng(),
            visitor_count,
        }
    }
}

impl ChatServer {
    // 部屋のすべてのユーザーにメッセージを送信する
    fn send_message(&self, room: &str, message: &str, skip_id: usize) {
        if let Some(sessions) = self.rooms.get(room) {
            for id in sessions {
                if *id != skip_id {
                    if let Some(addr) = self.sessions.get(id) {
                        let _ = addr.do_send(Message(message.to_owned()));
                    }
                }
            }
        }
    }
}

// Make Actor for 'CharServer'
impl Actor for ChatServer {
    // 単純なコンテキストを使用します。
    // 必要なのは、他のアクターと通信する機能だけです。
    type Context = Context<Self>;
}

// Handler for Message
//
// 新しいセッションを登録し、このセッションに一意のIDを割り当てます
impl Handler<Connect> for ChatServer {
    type Result = usize;

    fn handle(&mut self, msg: Connect, _: &mut Context<Self>) -> Self::Result {
        println!("Semeone joined!!");

        // 同じ部屋にいるすべてのユーザーに通知する
        self.send_message(&"Main".to_owned(), "Someone joined!!", 0);

        // ランダムIDでセッションを登録する
        let id = self.rng.gen::<usize>();
        self.sessions.insert(id, msg.addr);

        // メインルームへの自動参加セッション
        self.rooms
        .entry("Main".to_owned())
        .or_insert_with(HashSet::new)
        .insert(id);

        let count = self.visitor_count.fetch_add(1, Ordering::SeqCst);
        self.send_message("Main", &format!("Total visitors {}", count), 0);

        // send id back
        id
    }
}

// Handler for Disconnect message.
impl Handler<Disconnect> for ChatServer {
    type Result = ();

    fn handle(&mut self, msg: Disconnect, _: &mut Context<Self>) {
        println!("Someone Disconnected!!");

        let mut rooms: Vec<String> = Vec::new();

        // remove address
        if self.sessions.remove(&msg.id).is_some() {
            // すべての部屋からセッションを削除
            for (name, sessions) in &mut self.rooms {
                if sessions.remove(&msg.id) {
                    rooms.push(name.to_owned());
                }
            }
        }

        // 他のユーザーにメッセージを送信する
        for room in rooms {
            self.send_message(&room, "Someone Disconnected!!", 0);
        }
    }
}

// Handler for Message message.
impl Handler<ClientMessage> for ChatServer {
    type Result = ();

    fn handle(&mut self, msg: ClientMessage, _: &mut Context<Self>) {
        self.send_message(&msg.room, msg.msg.as_str(), msg.id);
    }
}


// Handler for 'Listrooms' message
impl Handler<ListRooms> for ChatServer {
    type Result = MessageResult<ListRooms>;

    fn handle(&mut self, _: ListRooms, _: &mut Context<Self>) -> Self::Result {
        let mut rooms = Vec::new();

        for key in self.rooms.keys() {
            rooms.push(key.to_owned())
        }

        MessageResult(rooms)
    }
}


// 部屋に参加し、古い部屋に切断メッセージを送信します新しい部屋に参加メッセージを送信します
impl Handler<Join> for ChatServer {
    type Result = ();

    fn handle(&mut self, msg: Join, _: &mut Context<Self>) {
        let Join { id, name } = msg;
        let mut rooms = Vec::new();

        // すべての部屋からセッションを削除する
        for (n, sessions) in &mut self.rooms {
            if sessions.remove(&id) {
                rooms.push(n.to_owned());
            }
        }
        // 他のユーザーにメッセージを送信する
        for room in rooms {
            self.send_message(&room, "Someone Disconnected!!", 0);
        }

        self.rooms
        .entry(name.clone())
        .or_insert_with(HashSet::new)
        .insert(id);

        self.send_message(&name, "Someone Connected!!", 0);
    }
}