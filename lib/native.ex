defmodule IrohEx.Native do

  use Rustler,
    otp_app: :iroh_ex,
    crate: :iroh_ex

  version = Mix.Project.config()[:version]

  # use RustlerPrecompiled,
  #   otp_app: :iroh_ex,
  #   crate: :iroh_ex,
  #   base_url: "https://github.com/adiibanez/iroh_ex/releases/download/v#{version}",
  #   force_build: System.get_env("RUSTLER_IROHEX_BUILD") in ["1", "true"],
  #   version: version,
  #   max_retries: 0,
  #   targets: [
  #     "aarch64-apple-darwin",
  #     "x86_64-apple-darwin",
  #     # "aarch64-apple-ios-sim",
  #     # "aarch64-apple-ios",
  #     # "x86_64-apple-ios",
  #     "aarch64-unknown-linux-gnu",
  #     # "aarch64-unknown-linux-musl",
  #     "x86_64-pc-windows-msvc",
  #     "x86_64-unknown-linux-gnu"
  #     # "x86_64-unknown-linux-musl"
  #   ]

  # skip_compilation?: true,
  # load_from: {:iroh_ex, "priv/native/libiroh_ex"}

  # skip_compilation?: true

  _version = Mix.Project.config()[:version]

  @spec add(number(), number()) :: {:ok, number()} | {:error, term()}
  def add(_a, _b), do: error()

  @spec create_node(pid()) :: {:ok, reference()} | {:error, term()}
  def create_node(_pid), do: error()

  @spec create_node_async(pid()) :: {:ok, reference()} | {:error, term()}
  def create_node_async(_pid), do: error()

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

  @spec generate_secretkey() :: {:ok, binary()} | {:error, term()}
  def generate_secretkey(), do: error()

  def list_peers(_node), do: error()

  def disconnect_node(_node), do: error()

  ## Handle NIF errors when Rust module isn't loaded
  defp error, do: :erlang.nif_error(:nif_not_loaded)
end
