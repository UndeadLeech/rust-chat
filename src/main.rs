extern crate websocket;

use std::sync::{Arc, Mutex};
use std::sync::mpsc;
use std::thread;
use std::str;

use websocket::{Server, Message, Sender, Receiver};
use websocket::header::WebSocketProtocol;
use websocket::message::Type;


fn main() {
	let server = Server::bind("0.0.0.0:2794").unwrap();

	let (dispatcher_tx, dispatcher_rx) = mpsc::channel::<String>();
	let client_senders: Arc<Mutex<Vec<mpsc::Sender<String>>>> = Arc::new(Mutex::new(vec![]));

	// Dispatcher thread
	{
		let client_senders = client_senders.clone();
		let _ = thread::Builder::new().name("dispatcher".to_string()).spawn(move || {
			while let Ok(msg) = dispatcher_rx.recv() {
				let mut removed_clients = Vec::new();
				for (i, sender) in client_senders.lock().unwrap().iter().enumerate() {
					match sender.send(msg.clone()) {
						Ok(_) => {},
						Err(_) => {
							// Couldn't send message because client thread closed
							removed_clients.push(i);
						}
					}
				}

				// Remove all clients that don't exist anymore
				let mut removed_nbr = 0;
				for i in removed_clients {
					client_senders.lock().unwrap().swap_remove(i - removed_nbr);
					removed_nbr += 1;
				}
			}
		});
	}


	// client threads
	for connection in server {
		let dispatcher = dispatcher_tx.clone();
		let (client_tx, client_rx) = mpsc::channel();
		client_senders.lock().unwrap().push(client_tx);   // THIS MIGHT CAUSE THE POISONERROR!!!

		// Spawn a new thread for each connection.
		let _ = thread::Builder::new().name("client".to_string()).spawn(move || {
			let request = connection.unwrap().read_request().unwrap(); // Get the request
			let headers = request.headers.clone(); // Keep the headers so we can check them

			request.validate().unwrap(); // Validate the request

			let mut response = request.accept(); // Form a response

			if let Some(&WebSocketProtocol(ref protocols)) = headers.get() {
				if protocols.contains(&("rust-websocket".to_string())) {
					// We have a protocol we want to use
					response.headers.set(WebSocketProtocol(vec!["rust-websocket".to_string()]));
				}
			}

			let mut client = response.send().unwrap(); // Send the response

			let ip = client.get_mut_sender()
				.get_mut()
				.peer_addr()
				.unwrap();

			println!("Connection from {}", ip);

			let message: Message = Message::text("SERVER: Connected.".to_string());
			client.send_message(&message).unwrap();

			let (mut sender, mut receiver) = client.split();

			let mut username = String::new();

			let(tx, rx) = mpsc::channel::<Message>();
			let _ = thread::Builder::new().name("receiver".to_string()).spawn(move || {
				for message in receiver.incoming_messages() {
					if let Ok(message) = message {
						let message: Message = message;
						match message.opcode {
							Type::Close => {
								tx.send(message).unwrap();
								break;
							},
							_ => {
								tx.send(message).unwrap();
							}
						}
					}
					else {
						let message = Message::close();
						tx.send(message).unwrap();
						break;
					}
				}
			});

			loop {
				if let Ok(message) = rx.try_recv() {
					match message.opcode {
						Type::Close => {
							// Closes connection on both sides, doesn't check if client is still connected
							let message = Message::close();
							let _ = sender.send_message(&message);
							println!("Client {} disconnected", ip);
							return;
						},
						Type::Ping => {
							// This could be an issue if ping is sent and client dies, the server will panic!
							let message = Message::pong(message.payload);
							sender.send_message(&message).unwrap();
						},
						_ => {
							if username.is_empty() {
								let payload_bytes = &message.payload;
								let mut payload_string = match str::from_utf8(payload_bytes) {
									Ok(v) => v,
									Err(e) => panic!("Invalid UTF-8 sequence: {}", e),
								};
								if payload_string.len() > 7 {
									match &payload_string[..7] {
										"/login " => {
											// Confirms login, if client dies the server doesnt give a fuck
											payload_string = &payload_string[7..].trim();
											username = String::from(payload_string);
											if !username.is_empty() {
												let msg_end = "'.".to_string();
												let msg_string = "SERVER: Logged in as '".to_string() + &username + &msg_end;
												let message: Message = Message::text(&msg_string[..	]);
												let _ = sender.send_message(&message);
											}
										},
										_ => { }
									}
								}
							}
							else {
								let payload_bytes = &message.payload;
								let payload_string = match str::from_utf8(payload_bytes) {
									Ok(v) => v,
									Err(e) => panic!("Invalid UTF-8 sequence: {}", e),
								};
								let payload_string = payload_string.trim();
								if payload_string.len() > 0 {
									let msg_string = format!("MESSAGE: {}: {}", username, payload_string);
									dispatcher.send(msg_string).unwrap();
								}
							}
						}
					}
				}
				if let Ok(message) = client_rx.try_recv() {
					// Send message from dispatcher to sender, the server doesnt give a fuck when client is dead
					let message: Message = Message::text(message);
					let _ = sender.send_message(&message);
				}
			}
		});
	}
}
