def main(args)
  HTTP.server("127.0.0.1:8099", fn(request) -> %"Hello, world!"
    when request.method == "GET" && request.path == "/plaintext"
  when request.method == "GET" && request.path == "/json"
    %{"message" => "Hello, World!"}
  otherwise
    {"error" => "Not Found", "code" => 404}
  end
end)
end