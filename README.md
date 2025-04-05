# IrohEx
![2000 node iroh swarm](https://raw.githubusercontent.com/adiibanez/iroh_ex/refs/heads/main/media/iroh_swarm.jpg)

Attempt at a seamless Iroh integration via Elixir + Rustler.
Elixir orchestrates and manages concurrency, while Rust handles low-level networking, swarm-forming, encryption, and performance-critical tasks.
Currently includes only basic Iroh functionality — with randomized PKI and gossip topic assignment.

requires ulimit -n X configuration suitable to the number of nodes in the swarm. Default on osx is 256. This configuration isn't integrated in livebook at the moment. Starting many nodes ( thousands ) can upset your network stack and / or router.

[![Run in Livebook](https://livebook.dev/badge/v1/blue.svg)](https://livebook.dev/run?url=https://github.com/adiibanez/iroh_ex/blob/main/livebooks/sigmajs.livemd)

## Installation

If [available in Hex](https://hex.pm/docs/publish), the package can be installed
by adding `iroh_ex` to your list of dependencies in `mix.exs`:

```elixir
def deps do
  [
    {:iroh_ex, "~> 0.0.6"}
  ]
end
```

Documentation can be generated with [ExDoc](https://github.com/elixir-lang/ex_doc)
and published on [HexDocs](https://hexdocs.pm). Once published, the docs can
be found at <https://hexdocs.pm/iroh_ex>.
