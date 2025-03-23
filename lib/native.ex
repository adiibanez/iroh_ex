defmodule IrohEx.Native do
  use Rustler,
    otp_app: :iroh_ex,
    crate: :iroh_ex

  # skip_compilation?: true

  _version = Mix.Project.config()[:version]

  @spec add(number(), number()) :: {:ok, number()} | {:error, term()}
  def add(_a, _b), do: error()

  @spec create_node(pid()) :: {:ok, reference()} | {:error, term()}
  def create_node(_pid), do: error()

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

  ## Handle NIF errors when Rust module isn't loaded
  defp error, do: :erlang.nif_error(:nif_not_loaded)
end
