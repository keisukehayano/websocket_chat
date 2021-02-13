use std::sync::{
    atomic::{ AtomicUsize, Ordering },
    Arc,
};
use std::time::{ Duration, Instant };

use actix::*;
use actix_files as fs;
use actix_web::{ web, App, Error, HttpRequest, HttpResponse, HttpServer, Responder };
use actix_web_actors::ws;

mod server;

// ハートビートpingが送信される頻度
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
// クライアントの応答がないためにタイムアウトが発生するまでの時間
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

// WebSocketルートのエントリポイント
async fn chat_route(
    req: HttpRequest,
    stream: web::Payload,
    srv: web::Data<Addr<server::ChatServer>>,
) -> Result<HttpResponse, Error> {
    ws::start(
        WsChatSession {
            id: 0,
            hb: Instant::now(),
            room: "Main".to_owned(),
            name: None,
            addr: srv.get_ref().clone(),
        },
        &req,
        stream,
    )
}

// Displays and affects state
async fn get_count(count: web::Data<Arc<AtomicUsize>>) -> impl Responder {
    let current_count = count.fetch_add(1, Ordering::SeqCst);
    format!("Visitors: {}", current_count)
}

struct WsChatSession {
    // unique session id
    id: usize,
    // クライアントは、少なくとも10秒に1回（CLIENT_TIMEOUT）pingを送信する必要があります。
    // そうしないと、接続が切断されます。
    hb: Instant,
    // joined room
    room: String,
    // peer name
    name: Option<String>,
    addr: Addr<server::ChatServer>,
}

impl Actor for WsChatSession {
    type Context = ws::WebsocketContext<Self>;

    // アクターの開始時にメソッドが呼び出されます。
    // wsセッションをChatServerに登録します
    fn started(&mut self, ctx: &mut Self::Context) {
        // セッション開始時にハートビートプロセスを開始します。
        self.hb(ctx);

        // チャットサーバーに自分を登録します。 
        // `AsyncContext :: wait`はコンテキスト内でfutureを登録しますが、
        // コンテキストはこのfutureが解決するまで待機してから、他のイベントを処理します。
        // HttpContext :: state（）はWsChatSessionStateのインスタンスであり、
        // 状態はアプリケーション内のすべてのルートで共有されます
        let addr = ctx.address();
        self.addr
            .send(server::Connect {
                addr: addr.recipient(),
            })
            .into_actor(self)
            .then(|res, act, ctx| {
                match res {
                    Ok(res) => act.id = res,
                    // チャットサーバーに問題があります
                    _ => ctx.stop(),
                }
                fut::ready(())
            })
            .wait(ctx);
    }

    fn stopping(&mut self, _: &mut Self::Context) -> Running {
        // notify chat server
        self.addr.do_send(server::Disconnect { id: self.id });
        Running::Stop
    }
}

// チャットサーバーからのメッセージを処理し、ピアWebSocketに送信するだけです
impl Handler<server::Message> for WsChatSession {
    type Result = ();

    fn handle(&mut self, msg: server::Message, ctx: &mut Self::Context) {
        ctx.text(msg.0);
    }
}

// WebSocket message handler
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsChatSession {
    fn handle(
        &mut self,
        msg: Result<ws::Message, ws::ProtocolError>,
        ctx: &mut Self::Context,
    ) {
        let msg = match msg {
            Err(_) =>  {
                ctx.stop();
                return;
            }
            Ok(msg) => msg,
        };

        println!("WEBSOCKET MESSAGE: {:?}", msg);

        match msg {
            ws::Message::Ping(msg) => {
                self.hb = Instant::now();
                ctx.pong(&msg);
            }
            ws::Message::Pong(_) => {
                self.hb = Instant::now();
            }
            ws::Message::Text(text) => {
                let m = text.trim();
                // we check for /sss type of messages
                if m.starts_with('/') {
                    let v: Vec<&str> = m.splitn(2, ' ').collect();
                    match v[0] {
                        "/list" => {
                            // ListRoomsメッセージをチャットサーバーに送信し、応答を待つ
                            println!("list rooms");
                            self.addr
                                .send(server::ListRooms)
                                .into_actor(self)
                                .then(| res, _, ctx | {
                                    match res {
                                        Ok(rooms) => {
                                            for room in rooms {
                                                ctx.text(room);
                                            }
                                        }
                                        _ => println!("Something Wrong!!"),
                                    }
                                    fut::ready(())
                                })
                                .wait(ctx)
                                // .wait（ctx）はコンテキスト内のすべてのイベントを一時停止するため、
                                // アクターは部屋のリストを取得するまで新しいメッセージを受信しません。                         
                        }
                        "/join" => {
                            if v.len() == 2 {
                                self.room = v[1].to_owned();
                                self.addr.do_send(server::Join {
                                    id: self.id,
                                    name: self.room.clone(),
                                });

                                ctx.text("joined!!");
                            } else {
                                ctx.text("!!! room name is requierd!!");
                            }
                        }
                        "/name" => {
                            if v.len() == 2 {
                                self.name = Some(v[1].to_owned());
                            } else {
                                ctx.text("!!! name is required!!");
                            }
                        }
                        _ => ctx.text(format!("!!! unknown command: {:?}", m)),
                    }
                } else {
                    let msg = if let Some(ref name) = self.name {
                        format!("{}: {}", name, m)
                    } else {
                        m.to_owned()
                    };
                    // チャットサーバーにメッセージを送信する
                    self.addr.do_send(server::ClientMessage {
                        id: self.id,
                        msg,
                        room: self.room.clone(),
                    })
                }
            }
            ws::Message::Binary(_) => println!("Unexpected binary"),
            ws::Message::Close(reason) => {
                ctx.close(reason);
                ctx.stop();
            }
            ws::Message::Continuation(_) => {
                ctx.stop();
            }
            ws::Message::Nop => (),
        }
    }
}

// ハートビート実装　
impl WsChatSession {
    // helper method that sends ping to client every second.
    //
    // also this method checks heartbeats from client
    fn hb(&self, ctx: &mut ws::WebsocketContext<Self>) {
        ctx.run_interval(HEARTBEAT_INTERVAL, |act, ctx| {
            // check client heartbeats
            if Instant::now().duration_since(act.hb) > CLIENT_TIMEOUT {
                // heartbeat timed out
                println!("Websocket Client haertbeat failed, disconnecting!!!!");

                // notify chat server
                act.addr.do_send(server::Disconnect { id: act.id });

                // ctx stop
                ctx.stop();

                // don't try send ping
                return;
            }

            ctx.ping(b"");
        });
    }
}


#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    // App state
    // 来場者数をカウントしています
    let app_state = Arc::new(AtomicUsize::new(0));

    // Start chat server actor
    let server = server::ChatServer::new(app_state.clone()).start();

    // Create Http server with websocket support
    HttpServer::new(move || {
        App::new()
            .data(app_state.clone())
            .data(server.clone())
            // redirect to websocket.html
            .service(web::resource("/").route(web::get().to(|| {
                HttpResponse::Found()
                .header("LOCATION", "/static/websocket.html")
                .finish()
            })))
            .route("/count/", web::get().to(get_count))
            // websocket
            .service(web::resource("/ws/").to(chat_route))
            // static resouces
            .service(fs::Files::new("/static/", "static/"))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}