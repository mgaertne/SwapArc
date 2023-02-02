# SwapArc

You can add SwapArc to your Cargo toml by copying this line:
`swap-arc = "0.1.0"`

Note: This is basically a faster (only tested on multiple x86_64 setups) drop-in replacement for `ArcSwap`

SwapArc allows you to swap out Arcs while using them. \
Let's consider this example:

```rust
use std::sync::Arc;
use std::thread;

struct Config {
    timout: u64,
    ...
}

struct Server {
    config: SwapArc<Config>,
    ...
}

fn test() {
    let server = Arc::new(Server {
        config: Config {
            timout: 1000,
            ...
        },
        ...
    });
    thread::spawn(|| {
        loop {
            // load the config without fearing any blocking or expensive operations.
            server.accept_connection(server.config.load().timeout);
        }
    });
    ...
}

// on network update, update the config seamlessly without blocking loads
server.config.update(Arc::new(Config {
    timeout: ...,
    ...
}));
```