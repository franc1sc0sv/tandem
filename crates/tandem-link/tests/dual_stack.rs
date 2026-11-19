//! T-196: one endpoint on `[::]` talks to IPv4 and IPv6 peers (the configured cable peer is
//! IPv4 link-local, the one learned by mDNS is IPv6 link-local).

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tandem_link::identity::{Identity, PublicKey};
use tandem_link::transport::{Endpoint, PeerTrust};

struct Keys(BTreeSet<PublicKey>);

impl PeerTrust for Keys {
    fn is_trusted(&self, key: &PublicKey) -> bool {
        self.0.contains(key)
    }
}

fn trusting(peers: &[&Identity]) -> Arc<dyn PeerTrust> {
    Arc::new(Keys(peers.iter().map(|i| i.public_key()).collect()))
}

async fn dial(client: &Endpoint, server: &Endpoint, addr: SocketAddr) {
    let (c, s) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(client.connect(addr), server.accept())
    })
    .await
    .expect("timed out");
    c.expect("dial");
    s.expect("accept").expect("handshake");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_endpoint_on_the_unspecified_ipv6_address_serves_ipv4_and_dials_ipv6() {
    let (a, b, c) = (
        Identity::from_seed([1; 32]),
        Identity::from_seed([2; 32]),
        Identity::from_seed([3; 32]),
    );
    let any6 = SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 0);
    let dual = Endpoint::bind(any6, &a, trusting(&[&b, &c])).unwrap();
    let port = dual.local_addr().unwrap().port();

    // An IPv4 peer reaches it (Windows sockets are IPv6-only unless told otherwise).
    let v4 = Endpoint::bind((Ipv4Addr::LOCALHOST, 0).into(), &b, trusting(&[&a])).unwrap();
    dial(&v4, &dual, (Ipv4Addr::LOCALHOST, port).into()).await;

    // And it dials an IPv6 peer: an IPv4-only endpoint fails here with
    // `InvalidRemoteAddress`, the error of the hardware run.
    let v6 = Endpoint::bind((Ipv6Addr::LOCALHOST, 0).into(), &c, trusting(&[&a])).unwrap();
    let to = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), v6.local_addr().unwrap().port());
    dial(&dual, &v6, to).await;
}
