use std::{
    io::{self, Read, Write},
    net::TcpStream,
};

#[derive(Debug)]
pub enum Connection {
    #[allow(dead_code)]
    Bluetooth,

    TcpStream(TcpStream),

    #[cfg(feature = "serial")]
    Serial(Box<dyn serialport::SerialPort>),

    /// The response `data` is built lazily on the first `write_all`, once
    /// the `id` of the outgoing request is known, so it matches whatever
    /// random id the request happened to be serialized with.
    #[cfg(test)]
    PartialReadTest {
        result: serde_cbor::Value,
        data: Vec<u8>,
        status: usize,
    },

    /// Simulates a stale response (answering some earlier, unrelated
    /// request) sitting in front of the real response in the transport
    /// buffer, as can happen on a quick reconnect.
    #[cfg(test)]
    StaleThenFreshTest {
        stale_result: serde_cbor::Value,
        result: serde_cbor::Value,
        data: Vec<u8>,
    },
}

#[cfg(test)]
fn extract_request_id(buf: &[u8]) -> String {
    let request: serde_cbor::Value =
        serde_cbor::from_slice(buf).expect("test request must be valid cbor");
    match &request {
        serde_cbor::Value::Map(map) => map
            .get(&serde_cbor::Value::Text("id".to_string()))
            .and_then(|v| match v {
                serde_cbor::Value::Text(s) => Some(s.clone()),
                _ => None,
            })
            .expect("request must have a string id"),
        _ => panic!("request must serialize to a cbor map"),
    }
}

impl Connection {
    pub fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Connection::Bluetooth => unimplemented!(),
            Connection::TcpStream(stream) => stream.write_all(buf),

            #[cfg(feature = "serial")]
            Connection::Serial(port) => port.write_all(buf),

            #[cfg(test)]
            Connection::PartialReadTest {
                result,
                data,
                status: _,
            } => {
                let id = extract_request_id(buf);
                let resp = crate::protocol::Response {
                    id,
                    result: Some(result.clone()),
                    error: None,
                };
                serde_cbor::to_writer(data, &resp).expect("response must serialize");
                Ok(())
            }

            #[cfg(test)]
            Connection::StaleThenFreshTest {
                stale_result,
                result,
                data,
            } => {
                let id = extract_request_id(buf);
                let stale = crate::protocol::Response {
                    id: format!("{id}-stale"),
                    result: Some(stale_result.clone()),
                    error: None,
                };
                let fresh = crate::protocol::Response {
                    id,
                    result: Some(result.clone()),
                    error: None,
                };
                serde_cbor::to_writer(&mut *data, &stale).expect("response must serialize");
                serde_cbor::to_writer(&mut *data, &fresh).expect("response must serialize");
                Ok(())
            }
        }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Connection::Bluetooth => todo!(),
            Connection::TcpStream(stream) => stream.read(buf),

            #[cfg(feature = "serial")]
            Connection::Serial(port) => port.read(buf),

            #[cfg(test)]
            Connection::PartialReadTest { data, status, .. } => match status {
                0 => {
                    buf[0] = data[0];
                    *status = 1;
                    Ok(1)
                }
                1 => {
                    *status = 2;
                    Err(io::Error::new(io::ErrorKind::Interrupted, "oh no!"))
                }
                _ => {
                    buf[..data.len() - 1].copy_from_slice(&data[1..]);
                    Ok(data.len() - 1)
                }
            },

            #[cfg(test)]
            Connection::StaleThenFreshTest { data, .. } => {
                let len = data.len();
                buf[..len].copy_from_slice(data);
                Ok(len)
            }
        }
    }
}

impl From<TcpStream> for Connection {
    fn from(stream: TcpStream) -> Self {
        Connection::TcpStream(stream)
    }
}

#[cfg(feature = "serial")]
impl From<Box<dyn serialport::SerialPort>> for Connection {
    fn from(port: Box<dyn serialport::SerialPort>) -> Self {
        Connection::Serial(port)
    }
}

#[cfg(test)]
mod test {

    use lwk_common::Network;
    use serde_cbor::Value;

    use crate::{protocol::Request, Jade};

    use super::Connection;

    #[test]
    fn partial_read() {
        let text = Value::Text("Hello".to_string());

        let connection = Connection::PartialReadTest {
            result: text.clone(),
            data: Vec::new(),
            status: 0,
        };

        let jade = Jade::new(connection, Network::default_regtest());
        let result: Value = jade.send(Request::Ping).unwrap();
        assert_eq!(result, text);
    }

    /// Reproduces the race from a quick reconnect: a response to some
    /// earlier, unrelated request is still sitting in the transport buffer
    /// ahead of the answer to the request we just sent. The stale response
    /// must be discarded by `id`, not force-decoded as if it answered the
    /// current request.
    #[test]
    fn stale_response_is_discarded() {
        let stale = Value::Bool(true);
        let fresh = Value::Text("Hello".to_string());

        let connection = Connection::StaleThenFreshTest {
            stale_result: stale,
            result: fresh.clone(),
            data: Vec::new(),
        };

        let jade = Jade::new(connection, Network::default_regtest());
        let result: Value = jade.send(Request::Ping).unwrap();
        assert_eq!(result, fresh);
    }
}
