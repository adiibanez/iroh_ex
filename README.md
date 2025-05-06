# IrohEx
![2000 node iroh swarm](https://raw.githubusercontent.com/adiibanez/iroh_ex/refs/heads/main/media/iroh_swarm.jpg)

Attempt at a seamless Iroh integration via Elixir + Rustler.
Elixir orchestrates and manages concurrency, while Rust handles low-level networking, swarm-forming, encryption, and performance-critical tasks.
Currently includes only basic Iroh functionality — with randomized PKI and gossip topic assignment.

No erlang distribution features leveraged so far; docker-compose setup with multiple nodes instead of running all nodes on one erlang instance is WIP. Starting many nodes ( thousands ) on one machine can easily upset your network stack and / or consumer grade router. It requires careful filedescriptor / ulimit -n X configuration suitable to the number of nodes in the swarm ( 5 peer connections + relay connection per node ). Also the tests with up to ~4000 nodes were configured with a local relay server instead of public iroh servers. Default ulimit on osx is 256. This configuration isn't integrated in livebook at the moment and needs to be executed before starting livebook. 

[![Run in Livebook](https://livebook.dev/badge/v1/blue.svg)](https://livebook.dev/run?url=https://github.com/adiibanez/iroh_ex/blob/main/livebooks/sigmajs.livemd)

[Highres png and videos](https://github.com/adiibanez/iroh_ex/releases/tag/MEDIA)

## Installation

If [available in Hex](https://hex.pm/docs/publish), the package can be installed
by adding `iroh_ex` to your list of dependencies in `mix.exs`:

```elixir
def deps do
  [
    {:iroh_ex, "~> 0.0.10"}
  ]
end
```

Documentation can be generated with [ExDoc](https://github.com/elixir-lang/ex_doc)
and published on [HexDocs](https://hexdocs.pm). Once published, the docs can
be found at <https://hexdocs.pm/iroh_ex>.
