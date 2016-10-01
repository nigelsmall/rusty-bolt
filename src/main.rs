mod packstream;
use packstream::{Value, Unpacker, Packer};

use std::vec::Vec;
use std::collections::HashMap;
use std::io::prelude::*;
use std::net::TcpStream;

const BOLT: [u8; 4] = [0x60, 0x60, 0xB0, 0x17];
const RAW_BOLT_VERSIONS: [u8; 16] = [0x00, 0x00, 0x00, 0x01,
                                     0x00, 0x00, 0x00, 0x00,
                                     0x00, 0x00, 0x00, 0x00,
                                     0x00, 0x00, 0x00, 0x00];

fn connect(address: &str) -> TcpStream {
    let mut stream = TcpStream::connect(address).unwrap();

    let _ = stream.write(&BOLT);
    let _ = stream.write(&RAW_BOLT_VERSIONS);
    let mut buf = [0; 4];
    let result = stream.read(&mut buf);
    match result {
        Ok(_) => {
            let version: u32 = (buf[0] as u32) << 24 |
                               (buf[1] as u32) << 16 |
                               (buf[2] as u32) << 8 |
                               (buf[3] as u32);
            println!("Using Bolt v{}", version)
        },
        Err(e) => panic!("Got an error: {}", e),
    }
    return stream;
}

const MAX_CHUNK_SIZE: usize = 0xFFFF;
const USER_AGENT: &'static str = "rusty-bolt/0.1.0";

macro_rules! log(
    ($($arg:tt)*) => { {
        let r = write!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    } }
);

macro_rules! log_line(
    ($($arg:tt)*) => { {
        let r = writeln!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    } }
);

struct BoltStream<'t> {
    stream: &'t mut TcpStream,
    packer: Packer,
    packer_marks: Vec<usize>,
    unpacker: Unpacker,
}

impl<'t> BoltStream<'t> {
    fn new(stream: &mut TcpStream) -> BoltStream {
        BoltStream { stream: stream, packer: Packer::new(), packer_marks: vec!(), unpacker: Unpacker::new()}
    }

    fn send(&mut self) {
        let mut offset: usize = 0;
        for &mark in &self.packer_marks {
            let size: usize = mark - offset;
            // TODO: size > MAX_CHUNK_SIZE
            let chunk_header = [(size >> 8) as u8, size as u8];
            let chunk_data = self.packer.get_chunk(offset, mark);
            let _ = self.stream.write(&chunk_header).unwrap();
            log!("C: [{:02X} {:02X}]", chunk_header[0], chunk_header[1]);
            let _ = self.stream.write(&chunk_data).unwrap();
            for i in 0..chunk_data.len() {
                log!(" {:02X}", chunk_data[i]);
            }
            let _ = self.stream.write(&[0, 0]).unwrap();
            log_line!(" [00 00]");
            offset = mark;
        }
        self.packer.clear();
        self.packer_marks.clear();
    }

    fn _fetch_chunk_size(&mut self) -> usize {
        let mut chunk_header = &mut [0u8; 2];
        let _ = self.stream.read_exact(chunk_header);
        log_line!("S: [{:02X} {:02X}]", chunk_header[0] as u8, chunk_header[1] as u8);
        0x100 * chunk_header[0] as usize + chunk_header[1] as usize
    }

    /**
     * Read the next message from the stream into the read buffer.
     */
    fn fetch_message(&mut self, response: &mut Response) {
        self.unpacker.clear();
        let mut chunk_size: usize = self._fetch_chunk_size();
        while chunk_size > 0 {
            let _ = self.stream.read_exact(&mut self.unpacker.buffer(chunk_size));
            chunk_size = self._fetch_chunk_size();
        }

        let message: Value = self.unpacker.unpack();
        match message {
            Value::Structure { signature, fields } => {
                match signature {
                    0x70 => {
                        match fields[0] {  // TODO: handle not enough fields
                            Value::Map(ref metadata) => response.on_success(metadata),
                            _ => panic!("SUCCESS metadata is not a map"),
                        }
                    },
                    0x71 => {
                        match fields[0] {  // TODO: handle not enough fields
                            Value::List(ref data) => response.on_record(data),
                            _ => panic!("RECORD data is not a list"),
                        }
                    },
                    0x7E => {
                        match fields[0] {  // TODO: handle not enough fields
                            Value::Map(ref metadata) => response.on_ignored(metadata),
                            _ => panic!("IGNORED metadata is not a map"),
                        }
                    },
                    0x7F => {
                        match fields[0] {  // TODO: handle not enough fields
                            Value::Map(ref metadata) => response.on_failure(metadata),
                            _ => panic!("FAILURE metadata is not a map"),
                        }
                    },
                    _ => panic!("Unknown response message with signature {:02X}", signature),
                }
            },
            _ => panic!("Response message is not a structure"),
        }
    }

    fn pack_init(&mut self, user: &str, password: &str) {
        self.packer.pack_structure_header(2, 0x01);
        self.packer.pack_string(USER_AGENT);
        self.packer.pack_map_header(3);
        self.packer.pack_string("scheme");
        self.packer.pack_string("basic");
        self.packer.pack_string("principal");
        self.packer.pack_string(user);
        self.packer.pack_string("credentials");
        self.packer.pack_string(password);
        self.packer_marks.push(self.packer.len());
    }

    fn pack_run(&mut self, statement: &str) {
        self.packer.pack_structure_header(2, 0x10);
        self.packer.pack_string(statement);
        self.packer.pack_map_header(0);
        self.packer_marks.push(self.packer.len());
    }

    fn pack_pull_all(&mut self) {
        self.packer.pack_structure_header(0, 0x3F);
        self.packer_marks.push(self.packer.len());
    }

}

trait Response {
    fn on_success(&mut self, metadata: &HashMap<String, Value>);
    fn on_record(&mut self, data: &Vec<Value>);
    fn on_ignored(&mut self, metadata: &HashMap<String, Value>);
    fn on_failure(&mut self, metadata: &HashMap<String, Value>);
}

struct DumpingResponse {
}

impl Response for DumpingResponse {
    fn on_success(&mut self, metadata: &HashMap<String, Value>) {
        println!("S: SUCCESS {:?}", metadata);
    }

    fn on_record(&mut self, data: &Vec<Value>) {
        println!("S: RECORD {:?}", data);
    }

    fn on_ignored(&mut self, metadata: &HashMap<String, Value>) {
        println!("S: IGNORED {:?}", metadata);
    }

    fn on_failure(&mut self, metadata: &HashMap<String, Value>) {
        println!("S: FAILURE {:?}", metadata);
    }
}

fn main() {
    let mut out = connect("127.0.0.1:7687");
    let mut bolt = BoltStream::new(&mut out);
    let mut response = &mut DumpingResponse {};

    bolt.pack_init("neo4j", "password");
    bolt.send();
    bolt.fetch_message(response);

    bolt.pack_run("UNWIND range(1, 3) AS n RETURN n");
    bolt.pack_pull_all();
    bolt.send();
    bolt.fetch_message(response);  // SUCCESS (RUN)
    bolt.fetch_message(response);  // RECORD
    bolt.fetch_message(response);  // RECORD
    bolt.fetch_message(response);  // RECORD
    bolt.fetch_message(response);  // SUCCESS (PULL_ALL)

}
