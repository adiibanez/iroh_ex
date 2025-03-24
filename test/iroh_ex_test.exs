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

    nodes = create_nodes_parallel(1)

    # nodes =
    #   Enum.map(1..count, fn _ ->
    #     Native.create_node(self())
    #   end)

    tasks =
      Enum.map(nodes, fn node ->
        Task.async(fn -> Native.connect_node(node, ticket) end)
      end)

    Enum.each(tasks, &Task.await/1)

    # Process.sleep(1000)

    Enum.each(1..10, fn x ->
      node = Enum.random(nodes)

      node
      |> Native.send_message("Elixir: Message #{inspect(x)}")

      # Task.async(fn ->
      #   Process.sleep(:rand.uniform(1000))
      #   Native.send_message(node, "Elixir: Message #{inspect(x)}")
      # end)
    end)

    Process.sleep(5000)

    assert IrohEx.hello() == :world
  end

  def create_nodes_parallel(count) when is_integer(count) and count > 0 do
    1..count
    # Launch tasks in parallel
    |> Enum.map(fn _ -> Task.async(fn -> Native.create_node(self()) end) end)
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
    # Maintain original order
    |> Enum.reverse()
  end
end
