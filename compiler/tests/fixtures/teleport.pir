# PIR Golden Fixture: teleport
# Quantum teleportation protocol
# Alice has |ψ⟩ = α|0⟩ + β|1⟩, shares Bell pair |Φ⁺⟩ with Bob
# Alice measures in Bell basis, sends 2 classical bits to Bob
# Bob applies X^b1 Z^b2 to recover |ψ⟩

[parameters]
N = 3  # 3 qubits: ψ, Alice's Bell, Bob's Bell

[domain teleport_domain]
dims = 0
n_iter = 0
n_param = 0
constraints = []

[schedule_tree]
root = Sequence {
  children = [
    # Prepare Bell pair |Φ⁺⟩ = (|00⟩ + |11⟩)/√2
    Domain {
      stmt_id = S_prepare_bell
      domain = teleport_domain
    },
    # Alice's CNOT and H on her qubit
    Domain {
      stmt_id = S_alice_ops
      domain = teleport_domain
    },
    # Alice measures in Bell basis (2 classical bits)
    Domain {
      stmt_id = S_alice_measure
      domain = teleport_domain
    },
    # Bob applies corrections based on measurement results
    Domain {
      stmt_id = S_bob_corrections
      domain = teleport_domain
    }
  ]
}

[statements]
S_prepare_bell = {
  domain = teleport_domain
  body = "H q[1]; CNOT q[1], q[2]"
  quantity = One
  mutability = Mutable
}
S_alice_ops = {
  domain = teleport_domain
  body = "CNOT q[0], q[1]; H q[0]"
  quantity = One
  mutability = Mutable
}
S_alice_measure = {
  domain = teleport_domain
  body = "b0 = measure q[0]; b1 = measure q[1]"
  quantity = Zero
  mutability = Immutable
}
S_bob_corrections = {
  domain = teleport_domain
  body = "if b1 { X q[2] }; if b0 { Z q[2] }"
  quantity = One
  mutability = Mutable
}

[accesses]
S_prepare_bell_q1 = {
  stmt_id = S_prepare_bell
  stmt_domain = teleport_domain
  map = { pieces = [{ domain = teleport_domain, matrix = [[]], constant = [1] }] }
  access_type = ReadWrite
  array_name = "q"
}
S_prepare_bell_q2 = {
  stmt_id = S_prepare_bell
  stmt_domain = teleport_domain
  map = { pieces = [{ domain = teleport_domain, matrix = [[]], constant = [2] }] }
  access_type = ReadWrite
  array_name = "q"
}
S_alice_ops_q0 = {
  stmt_id = S_alice_ops
  stmt_domain = teleport_domain
  map = { pieces = [{ domain = teleport_domain, matrix = [[]], constant = [0] }] }
  access_type = ReadWrite
  array_name = "q"
}
S_alice_ops_q1 = {
  stmt_id = S_alice_ops
  stmt_domain = teleport_domain
  map = { pieces = [{ domain = teleport_domain, matrix = [[]], constant = [1] }] }
  access_type = ReadWrite
  array_name = "q"
}
S_alice_measure_q0 = {
  stmt_id = S_alice_measure
  stmt_domain = teleport_domain
  map = { pieces = [{ domain = teleport_domain, matrix = [[]], constant = [0] }] }
  access_type = Read
  array_name = "q"
}
S_alice_measure_q1 = {
  stmt_id = S_alice_measure
  stmt_domain = teleport_domain
  map = { pieces = [{ domain = teleport_domain, matrix = [[]], constant = [1] }] }
  access_type = Read
  array_name = "q"
}
S_bob_corrections_q2 = {
  stmt_id = S_bob_corrections
  stmt_domain = teleport_domain
  map = { pieces = [{ domain = teleport_domain, matrix = [[]], constant = [2] }] }
  access_type = ReadWrite
  array_name = "q"
}

[quantities]
#
# These are QUBITS, and a qubit is not a linear value in the consume-once sense.
#
# They were annotated `One`, which means "consumed exactly once", and that is wrong
# for a qubit. Teleportation applies 3-4 gates to each of these three qubits:
#
#     q[0]: CNOT, H, measure                              -- 3 uses
#     q[1]: H, CNOT, CNOT, measure                        -- 4 uses
#     q[2]: CNOT, X, Z                                    -- 3 uses
#
# A gate does not CONSUME a qubit, it entangles it and hands it on. `One` is the
# annotation for a resource that leaves the program when used -- a classical bit, an
# allocated buffer -- and a qubit is the opposite: it persists and is reused.
#
# This was not visible while the fixture's `[1]` claim went unchecked. The quantum
# operand names in the bodies (`H q[1]`) were DISCARDED by the fixture parser, which
# replaced every gate operand with a fresh anonymous `qir.qubit_alloc()`. So the three
# declared names appeared zero times in the lowered body, and a linearity check
# counting occurrences of `q[0]` correctly reported `used 0 times`.
#
# The check was right and the fixture was unsound: it declared named linear qubits
# and then modelled gates on anonymous allocations, which is precisely the
# destroy-and-reuse failure QTT exists to prevent -- the gates were not operating on
# the declared qubits at all.
#
# `Many` is the honest annotation: a qubit wire may be used any number of times.
# Consuming a qubit is a separate, explicit act (measurement / release), and that is
# where `[1]` does belong -- on the measured bits below, not on the wires.
q[0] = Many
q[1] = Many
q[2] = Many
b0 = Zero
b1 = Zero