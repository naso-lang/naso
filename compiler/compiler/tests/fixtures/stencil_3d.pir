# PIR Golden Fixture: stencil_3d
# 3D 7-point stencil: u[i][j][k] = (u[i-1][j][k] + u[i+1][j][k] + u[i][j-1][k] + u[i][j+1][k] + u[i][j][k-1] + u[i][j][k+1] + u[i][j][k]) / 7

[parameters]
N = 128
M = 128
K = 128

[loops]
# The loop nest this kernel's bodies are written against. The harness builds a
# real Band from these names and the statement's own domain, so `i`, `j`, `k`
# are bound induction variables rather than unbound reads.
#
# Without this the schedule was a flat Sequence of domains, so no loop existed,
# and every `A[i][k]` silently read as i64 zero.
iterators = i, j, k

[function_params]
# The ABI slots this kernel's arrays occupy. A PIR fixture has no function
# structure, so the arrays a body subscripts must be declared here to be
# bindable; an undeclared subscripted name is a refusal, not a zero.
#
# `f64`/`i64` is the element type and the numbers are the shape, outermost
# first. `twiddle[stage][k]` needs two extents because linearising it is
# `stage * 1024 + k`, and a shape is what makes that a GEP.
u = Tensor[f64, 16, 16, 16]

[domain stencil_domain]
dims = 3
n_iter = 3
n_param = 0
constraints = [
  [1, 0, 0, 1],      # i >= 1
  [-1, 0, 0, 126],   # i <= 126
  [0, 1, 0, 1],      # j >= 1
  [0, -1, 0, 126],   # j <= 126
  [0, 0, 1, 1],      # k >= 1
  [0, 0, -1, 126],   # k <= 126
]

[schedule_tree]
root = Band {
  members = [
    { matrix = [[1, 0, 0]], constant = [0] },    # i loop
  ]
  coincident = [false]
  child = Band {
    members = [
      { matrix = [[0, 1, 0]], constant = [0] },  # j loop
    ]
    coincident = [true]
    child = Band {
      members = [
        { matrix = [[0, 0, 1]], constant = [0] },  # k loop
      ]
      coincident = [true]
      child = Domain {
        stmt_id = S0
        domain = stencil_domain
      }
    }
  }
}

[statements]
S0 = {
  domain = stencil_domain
  body = "u[i][j][k] = (u[i-1][j][k] + u[i+1][j][k] + u[i][j-1][k] + u[i][j+1][k] + u[i][j][k-1] + u[i][j][k+1] + u[i][j][k]) / 7"
  quantity = Many
  mutability = Immutable
}

[accesses]
S0_read_center = {
  stmt_id = S0
  stmt_domain = stencil_domain
  map = { pieces = [{ domain = stencil_domain, matrix = [[1, 0, 0], [0, 1, 0], [0, 0, 1]], constant = [0, 0, 0] }] }
  access_type = Read
  array_name = "u"
}
S0_read_i_m1 = {
  stmt_id = S0
  stmt_domain = stencil_domain
  map = { pieces = [{ domain = stencil_domain, matrix = [[1, 0, 0], [0, 1, 0], [0, 0, 1]], constant = [-1, 0, 0] }] }
  access_type = Read
  array_name = "u"
}
S0_read_i_p1 = {
  stmt_id = S0
  stmt_domain = stencil_domain
  map = { pieces = [{ domain = stencil_domain, matrix = [[1, 0, 0], [0, 1, 0], [0, 0, 1]], constant = [1, 0, 0] }] }
  access_type = Read
  array_name = "u"
}
S0_read_j_m1 = {
  stmt_id = S0
  stmt_domain = stencil_domain
  map = { pieces = [{ domain = stencil_domain, matrix = [[1, 0, 0], [0, 1, 0], [0, 0, 1]], constant = [0, -1, 0] }] }
  access_type = Read
  array_name = "u"
}
S0_read_j_p1 = {
  stmt_id = S0
  stmt_domain = stencil_domain
  map = { pieces = [{ domain = stencil_domain, matrix = [[1, 0, 0], [0, 1, 0], [0, 0, 1]], constant = [0, 1, 0] }] }
  access_type = Read
  array_name = "u"
}
S0_read_k_m1 = {
  stmt_id = S0
  stmt_domain = stencil_domain
  map = { pieces = [{ domain = stencil_domain, matrix = [[1, 0, 0], [0, 1, 0], [0, 0, 1]], constant = [0, 0, -1] }] }
  access_type = Read
  array_name = "u"
}
S0_read_k_p1 = {
  stmt_id = S0
  stmt_domain = stencil_domain
  map = { pieces = [{ domain = stencil_domain, matrix = [[1, 0, 0], [0, 1, 0], [0, 0, 1]], constant = [0, 0, 1] }] }
  access_type = Read
  array_name = "u"
}
S0_write = {
  stmt_id = S0
  stmt_domain = stencil_domain
  map = { pieces = [{ domain = stencil_domain, matrix = [[1, 0, 0], [0, 1, 0], [0, 0, 1]], constant = [0, 0, 0] }] }
  access_type = Write
  array_name = "u"
}

[quantities]
N = Zero
M = Zero
K = Zero