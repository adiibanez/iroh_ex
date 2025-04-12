defmodule IrohEx.Native do

  alias IrohEx.NodeConfig

  # use Rustler,
  #   otp_app: :iroh_ex,
  #   crate: :iroh_ex

  version = Mix.Project.config()[:version]

  use RustlerPrecompiled,
    otp_app: :iroh_ex,
    crate: :iroh_ex,
    base_url: "https://github.com/adiibanez/iroh_ex/releases/download/v#{version}",
    force_build: System.get_env("RUSTLER_IROHEX_BUILD") in ["1", "true"],
    version: version,
    max_retries: 0,
    targets: [
      "aarch64-apple-darwin",
      "x86_64-apple-darwin",
      # "aarch64-apple-ios-sim",
      # "aarch64-apple-ios",
      # "x86_64-apple-ios",
      "aarch64-unknown-linux-gnu",
      # "aarch64-unknown-linux-musl",
      "x86_64-pc-windows-msvc",
      "x86_64-unknown-linux-gnu"
      # "x86_64-unknown-linux-musl"
    ]

  # skip_compilation?: true,
  # load_from: {:iroh_ex, "priv/native/libiroh_ex"}

  # skip_compilation?: true

  _version = Mix.Project.config()[:version]

  @spec add(number(), number()) :: {:ok, number()} | {:error, term()}
  def add(_a, _b), do: error()

  @spec create_node(pid(), NodeConfig.t()) :: {:ok, reference()} | {:error, term()}
  def create_node(_pid, _node_config), do: error()

  @spec gen_node_addr(reference()) :: {:ok, binary()} | {:error, term()}
  def gen_node_addr(_node), do: error()

  @spec send_message(reference(), binary()) :: {:ok, reference()} | {:error, term()}
  def send_message(_node, _message), do: error()

  @spec create_ticket(reference()) :: {:ok, binary()} | {:error, term()}
  def create_ticket(_node), do: error()

  @spec connect_node_by_pubkey(binary()) :: {:ok, reference()} | {:error, term()}
  def connect_node_by_pubkey(_pubkey), do: error()

  @spec connect_node(reference(), binary()) :: {:ok, reference()} | {:error, term()}
  def connect_node(_node, _ticket), do: error()

  @spec cleanup(reference()) :: {:ok, reference()} | {:error, term()}
  def cleanup(_node), do: error()

  @spec generate_secretkey() :: {:ok, binary()} | {:error, term()}
  def generate_secretkey(), do: error()

  @spec generate_secretkey() :: {:ok, reference()} | {:error, term()}
  def list_peers(_node), do: error()

  @spec disconnect_node(reference()) :: {:ok} | {:error, term()}
  def disconnect_node(_node), do: error()

  @spec subscribe_to_topic(reference(), binary(), [binary()]) :: {:ok, reference()} | {:error, term()}
  def subscribe_to_topic(_node, _topic_str, _node_ids), do: error()

  @spec unsubscribe_from_topic(reference(), binary()) :: {:ok, reference()} | {:error, term()}
  def unsubscribe_from_topic(_node, _topic_str), do: error()

  @spec broadcast_message(reference(), binary(), binary()) :: {:ok, reference()} | {:error, term()}
  def broadcast_message(_node, _topic_str, _message), do: error()

  @spec list_topics(reference()) :: {:ok, [binary()]} | {:error, term()}
  def list_topics(_node), do: error()

  @spec shutdown(reference()) :: :ok | {:error, term()}
  def shutdown(_node), do: error()


  ## Handle NIF errors when Rust module isn't loaded
  defp error, do: :erlang.nif_error(:nif_not_loaded)
end

defmodule IrohEx.NodeConfig do
  @moduledoc false
  @enforce_keys [:is_whale_node, :active_view_capacity, :passive_view_capacity, :relay_urls, :discovery]
  defstruct [:is_whale_node, :active_view_capacity, :passive_view_capacity, :relay_urls, :discovery]

  @default_relay_urls ["https://euw1-1.relay.iroh.network./"]
  @default_discovery ["n0", "local_network"]

  @type t :: %__MODULE__{
    is_whale_node: boolean(),
    active_view_capacity: integer(),
    passive_view_capacity: integer(),
    relay_urls: [String.t()],
    discovery: [String.t()]
  }

  def build, do: %__MODULE__{
    is_whale_node: false,
    active_view_capacity: 10,
    passive_view_capacity: 10,
    relay_urls: @default_relay_urls,
    discovery: @default_discovery
  }
end
