; ModuleID = 'naso_module'
source_filename = "naso_module"

declare i64 @naso_write(i64, i8*, i64)

define void @naso_main() {
entry:
  %array_literal = alloca [5 x i8], align 1
  %array_elem_0 = getelementptr [5 x i8], [5 x i8]* %array_literal, i64 0, i64 0
  store i8 104, i8* %array_elem_0, align 1
  %array_elem_1 = getelementptr [5 x i8], [5 x i8]* %array_literal, i64 0, i64 1
  store i8 101, i8* %array_elem_1, align 1
  %array_elem_2 = getelementptr [5 x i8], [5 x i8]* %array_literal, i64 0, i64 2
  store i8 108, i8* %array_elem_2, align 1
  %array_elem_3 = getelementptr [5 x i8], [5 x i8]* %array_literal, i64 0, i64 3
  store i8 108, i8* %array_elem_3, align 1
  %array_elem_4 = getelementptr [5 x i8], [5 x i8]* %array_literal, i64 0, i64 4
  store i8 111, i8* %array_elem_4, align 1
  %array_elem_ptr = getelementptr [5 x i8], [5 x i8]* %array_literal, i64 0, i64 0
  %array_ptr_i64 = ptrtoint i8* %array_elem_ptr to i64
  %tensor_with_ptr = insertvalue { i64, i64 } undef, i64 %array_ptr_i64, 0
  %tensor_complete = insertvalue { i64, i64 } %tensor_with_ptr, i64 5, 1
  %body = alloca { i64, i64 }, align 8
  store { i64, i64 } %tensor_complete, { i64, i64 }* %body, align 4
  %body1 = load { i64, i64 }, { i64, i64 }* %body, align 4
  %tensor_ptr_cast = extractvalue { i64, i64 } %body1, 0
  %ffi_arg_ptr_1 = inttoptr i64 %tensor_ptr_cast to i8*
  %naso.call.naso_write = call i64 @naso_write(i64 1, i8* %ffi_arg_ptr_1, i64 5)
  ret void
}

define void @naso_entry() {
entry:
  call void @naso_main()
  ret void
}
