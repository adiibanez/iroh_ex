defmodule IrohExTest do
  use ExUnit.Case
  doctest IrohEx
  alias IrohEx.Native

  @node_cnt 100
  @msg_cnt 100_000
  @rand_msg_delay 100

  test "test iroh node" do
    node_ref = Native.create_node(self())
    ticket = Native.create_ticket(node_ref)

    node_id = Native.gen_node_addr(node_ref)

    _node_ref_connected = Native.connect_node(node_ref, ticket)

    IO.inspect(node_id, label: "Node id")
    assert is_binary(node_id)
  end

  test "test many iroh nodes" do
    mothership_node_ref = Native.create_node(self())

    ticket = Native.create_ticket(mothership_node_ref)

    IO.inspect(ticket, label: "Node1 ticket")

    # connect main node
    Task.async(fn -> Native.connect_node(mothership_node_ref, ticket) end)

    nodes = create_nodes(@node_cnt)

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

    # Process.sleep(10000)
    IO.puts("starting msg loop")

    tasks =
      Enum.map(1..@msg_cnt, fn x ->
        Task.async(fn ->
          # IO.puts("Nodes: #{Enum.count(nodes)}")
          # node = Enum.at(nodes, :rand.uniform(Enum.count(nodes) - 1))
          node = Enum.random(nodes)
          _node_id = Native.gen_node_addr(node)
          # IO.inspect(node, label: "Send msg Node ref")
          Process.sleep(:rand.uniform(@rand_msg_delay))
          # from #{node_id}
          Native.send_message(node, "#{x}")
        end)
      end)

    IO.inspect(Enum.count(tasks), label: "Tasks")

    Enum.each(tasks, &Task.await/1)
  end

  def create_nodes(node_count) when is_integer(node_count) and node_count > 0 do
    1..node_count
    |> Enum.map(fn _ ->
      Task.async(fn -> Native.create_node_async(self()) end)
    end)
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
    # |> dbg()
    # Maintain original order
    |> Enum.reverse()
  end
end
