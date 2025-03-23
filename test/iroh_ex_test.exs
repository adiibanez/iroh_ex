defmodule IrohExTest do
  use ExUnit.Case
  doctest IrohEx
  alias IrohEx.Native

  test "greets the world" do
    assert IrohEx.hello() == :world
  end

  test "create iroh endpoint" do
    mother_node_ref = Native.create_node(self())

    ticket = Native.create_ticket(mother_node_ref)

    IO.inspect(ticket, label: "Node1 ticket")

    node1_ref = Native.create_node(self())
    node2_ref = Native.create_node(self())
    node3_ref = Native.create_node(self())
    node4_ref = Native.create_node(self())

    assert is_reference(node1_ref)

    nodes = [node1_ref, node2_ref, node3_ref, node4_ref]

    Enum.each(nodes, fn node -> Native.connect_node(node, ticket) end)

    Process.sleep(2000)

    Enum.each(1..100, fn x ->
      node = Enum.random(nodes)

      node
      |> Native.send_message("Message #{inspect(x)}")
    end)

    Process.sleep(5000)

    assert IrohEx.hello() == :world
  end
end
