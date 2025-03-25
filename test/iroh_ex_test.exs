defmodule IrohExTest do
  use ExUnit.Case
  doctest IrohEx
  alias IrohEx.Native

  test "greets the world" do
    assert IrohEx.hello() == :world
  end

  @tag timeout: :infinity
  test "create iroh endpoint" do
    mother_node_ref = Native.create_node(self())

    ticket = Native.create_ticket(mother_node_ref)

    IO.inspect(ticket, label: "Node1 ticket")

    Task.async(fn -> Native.connect_node(mother_node_ref, ticket) end)

    nodes = create_nodes_parallel(50)

    IO.inspect(nodes, label: "Node list")

    # nodes =
    #   Enum.map(1..count, fn _ ->
    #     Native.create_node(self())
    #   end)

    tasks =
      Enum.map(nodes, fn n ->
        IO.inspect(n, label: "Connect Node ref")
        Task.async(fn -> Native.connect_node(n, ticket) end)
      end)

    Enum.each(tasks, &Task.await/1)

    Process.sleep(1000)

    Enum.each(1..5_000, fn x ->
      node = Enum.random(nodes)

      IO.inspect(node, label: "Send msg Node ref")

      # node
      # |> Native.send_message("Elixir: Message #{inspect(x)}")

      Task.async(fn ->
        Process.sleep(:rand.uniform(100))
        Native.send_message(node, "Elixir: Message #{inspect(x)}")
      end)
    end)

    Process.sleep(2000)

    assert IrohEx.hello() == :world
  end

  def create_nodes_parallel(count) when is_integer(count) and count > 0 do
    1..count
    # Launch tasks in parallel
    |> Enum.map(fn _ -> Task.async(fn -> Native.create_node_async(self()) end) end)
    # Await results
    |> Enum.map(&Task.await/1)
    |> Enum.reduce([], fn node_ref, acc ->
      case node_ref do
        # Collect valid references
        ref when is_reference(ref) ->
          [ref | acc]

        error ->
          IO.puts("Error creating node: #{inspect(error)}")
          # Skip errors
          acc
      end
    end)
    |> dbg()
    # Maintain original order
    |> Enum.reverse()
  end
end
