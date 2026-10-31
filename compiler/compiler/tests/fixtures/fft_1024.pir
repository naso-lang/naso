# PIR Golden Fixture: fft_1024
# Cooley-Tukey FFT with 1024 points
# Bit-reverse reorder + butterfly stages

[parameters]
N = 1024
LOGN = 10

[loops]
# The loop nest this kernel's bodies are written against. The harness builds a
# real Band from these names and the statement's own domain, so `i`, `j`, `k`
# are bound induction variables rather than unbound reads.
#
# Without this the schedule was a flat Sequence of domains, so no loop existed,
# and every `A[i][k]` silently read as i64 zero.
iterators = i, stage, k

[function_params]
# The ABI slots this kernel's arrays occupy. A PIR fixture has no function
# structure, so the arrays a body subscripts must be declared here to be
# bindable; an undeclared subscripted name is a refusal, not a zero.
#
# `f64`/`i64` is the element type and the numbers are the shape, outermost
# first. `twiddle[stage][k]` needs two extents because linearising it is
# `stage * 1024 + k`, and a shape is what makes that a GEP.
x = Tensor[f64, 1024]
twiddle = Tensor[f64, 10, 1024]

[domain bitrev_domain]
dims = 1
n_iter = 1
n_param = 0
constraints = [
  [1, 0],      # i >= 0
  [-1, 1023],  # i <= 1023
]

[domain butterfly_domain]
dims = 2
n_iter = 2
n_param = 0
constraints = [
  [1, 0, 0],       # stage >= 0
  [-1, 0, 9],      # stage <= 9
  [0, 1, 0],       # k >= 0
  [0, -1, 511],    # k <= 511 (N/2 - 1)
]

[schedule_tree]
root = Sequence {
  children = [
    # Bit-reverse permutation
    Band {
      members = [
        { matrix = [[1]], constant = [0] },
      ]
      coincident = [true]
      child = Domain {
        stmt_id = S_bitrev
        domain = bitrev_domain
      }
    },
    # Butterfly stages
    Band {
      members = [
        { matrix = [[1, 0]], constant = [0] },   # stage loop
      ]
      coincident = [false]
      child = Band {
        members = [
          { matrix = [[0, 1]], constant = [0] },  # k loop
        ]
        coincident = [true]
        child = Domain {
          stmt_id = S_butterfly
          domain = butterfly_domain
        }
      }
    }
  ]
}

[statements]
S_bitrev = {
  domain = bitrev_domain
  body = "x[i] <-> x[bitrev(i)]"
  quantity = Many
  mutability = Immutable
}
S_butterfly = {
  domain = butterfly_domain
  body = "butterfly(x[k], x[k + 2^stage], twiddle[stage][k])"
  quantity = Many
  mutability = Immutable
}

[accesses]
S_bitrev_read = {
  stmt_id = S_bitrev
  stmt_domain = bitrev_domain
  map = { pieces = [{ domain = bitrev_domain, matrix = [[1]], constant = [0] }] }
  access_type = ReadWrite
  array_name = "x"
}
S_butterfly_read1 = {
  stmt_id = S_butterfly
  stmt_domain = butterfly_domain
  map = { pieces = [{ domain = butterfly_domain, matrix = [[0, 1]], constant = [0] }] }
  access_type = ReadWrite
  array_name = "x"
}
S_butterfly_read2 = {
  stmt_id = S_butterfly
  stmt_domain = butterfly_domain
  map = { pieces = [{ domain = butterfly_domain, matrix = [[0, 1], [1, 0]], constant = [0, 0] }] }
  access_type = Read
  array_name = "x"
}
S_butterfly_twiddle = {
  stmt_id = S_butterfly
  stmt_domain = butterfly_domain
  map = { pieces = [{ domain = butterfly_domain, matrix = [[1, 0], [0, 1]], constant = [0, 0] }] }
  access_type = Read
  array_name = "twiddle"
}

[quantities]
N = Zero
LOGN = Zero