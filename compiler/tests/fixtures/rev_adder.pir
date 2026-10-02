# PIR Golden Fixture: rev_adder
# Reversible ripple-carry adder using Toffoli and CNOT gates
# Computes a + b -> a (reversible), requires ancilla for carry

[parameters]
N = 8  # 8-bit adder

[loops]
# The loop nest this kernel's bodies are written against. The harness builds a
# real Band from these names and the statement's own domain, so `i`, `j`, `k`
# are bound induction variables rather than unbound reads.
#
# Without this the schedule was a flat Sequence of domains, so no loop existed,
# and every `A[i][k]` silently read as i64 zero.
iterators = i

[function_params]
# The ABI slots this kernel's arrays occupy. A PIR fixture has no function
# structure, so the arrays a body subscripts must be declared here to be
# bindable; an undeclared subscripted name is a refusal, not a zero.
#
# `f64`/`i64` is the element type and the numbers are the shape, outermost
# first. `twiddle[stage][k]` needs two extents because linearising it is
# `stage * 1024 + k`, and a shape is what makes that a GEP.
a = Tensor[i64, 16]
b = Tensor[i64, 16]
carry = Tensor[i64, 17]
sum = Tensor[i64, 16]

[domain adder_domain]
dims = 1
n_iter = 1
n_param = 0
constraints = [
  [1, 0],      # i >= 0
  [-1, 7],     # i <= 7 (N-1)
]

[schedule_tree]
root = Sequence {
  children = [
    # Forward pass: compute sum and carry
    Band {
      members = [
        { matrix = [[1]], constant = [0] },
      ]
      coincident = [false]
      child = Domain {
        stmt_id = S_compute_sum
        domain = adder_domain
      }
    },
    # Uncompute carry ancilla
    Band {
      members = [
        { matrix = [[-1]], constant = [7] },  # reverse iteration
      ]
      coincident = [false]
      child = Domain {
        stmt_id = S_uncompute_carry
        domain = adder_domain
      }
    }
  ]
}

[statements]
S_compute_sum = {
  domain = adder_domain
  body = "sum[i] = a[i] ^ b[i] ^ carry[i]; carry[i+1] = majority(a[i], b[i], carry[i])"
  quantity = Many
  mutability = Mutable
}
S_uncompute_carry = {
  domain = adder_domain
  body = "uncompute carry[i+1]"
  quantity = Zero
  mutability = Immutable
}

[accesses]
S_compute_sum_a = {
  stmt_id = S_compute_sum
  stmt_domain = adder_domain
  map = { pieces = [{ domain = adder_domain, matrix = [[1]], constant = [0] }] }
  access_type = Read
  array_name = "a"
}
S_compute_sum_b = {
  stmt_id = S_compute_sum
  stmt_domain = adder_domain
  map = { pieces = [{ domain = adder_domain, matrix = [[1]], constant = [0] }] }
  access_type = Read
  array_name = "b"
}
S_compute_sum_carry_in = {
  stmt_id = S_compute_sum
  stmt_domain = adder_domain
  map = { pieces = [{ domain = adder_domain, matrix = [[1]], constant = [0] }] }
  access_type = Read
  array_name = "carry"
}
S_compute_sum_sum = {
  stmt_id = S_compute_sum
  stmt_domain = adder_domain
  map = { pieces = [{ domain = adder_domain, matrix = [[1]], constant = [0] }] }
  access_type = Write
  array_name = "sum"
}
S_compute_sum_carry_out = {
  stmt_id = S_compute_sum
  stmt_domain = adder_domain
  map = { pieces = [{ domain = adder_domain, matrix = [[1]], constant = [1] }] }
  access_type = Write
  array_name = "carry"
}
S_uncompute_carry = {
  stmt_id = S_uncompute_carry
  stmt_domain = adder_domain
  map = { pieces = [{ domain = adder_domain, matrix = [[1]], constant = [1] }] }
  access_type = ReadWrite
  array_name = "carry"
}

[quantities]
a = Many
b = Many
sum = Many
carry = Zero  # Ancilla, erased after uncomputation