; ModuleID = 'naso_module'
source_filename = "naso_module"

declare i64 @naso_write(i64, i8*, i64)

declare i64 @naso_read(i64, i8*, i64)

declare i64 @naso_tcp_bind(i8*, i64)

declare i64 @naso_tcp_accept(i64, i8*)

declare i64 @naso_accept_get_client(i8*)

declare i64 @naso_close(i64)

declare i64 @naso_runtime_new(i64)

declare i64 @naso_runtime_run(i8*)

declare i64 @naso_yield()

define void @naso_handle_client(i64 %client_fd) {
entry:
  %client_fd1 = alloca i64, align 8
  store i64 %client_fd, i64* %client_fd1, align 4
  %array_literal = alloca [16 x i8], align 1
  %array_elem_0 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 0
  store i8 0, i8* %array_elem_0, align 1
  %array_elem_1 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 1
  store i8 0, i8* %array_elem_1, align 1
  %array_elem_2 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 2
  store i8 0, i8* %array_elem_2, align 1
  %array_elem_3 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 3
  store i8 0, i8* %array_elem_3, align 1
  %array_elem_4 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 4
  store i8 0, i8* %array_elem_4, align 1
  %array_elem_5 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 5
  store i8 0, i8* %array_elem_5, align 1
  %array_elem_6 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 6
  store i8 0, i8* %array_elem_6, align 1
  %array_elem_7 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 7
  store i8 0, i8* %array_elem_7, align 1
  %array_elem_8 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 8
  store i8 0, i8* %array_elem_8, align 1
  %array_elem_9 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 9
  store i8 0, i8* %array_elem_9, align 1
  %array_elem_10 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 10
  store i8 0, i8* %array_elem_10, align 1
  %array_elem_11 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 11
  store i8 0, i8* %array_elem_11, align 1
  %array_elem_12 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 12
  store i8 0, i8* %array_elem_12, align 1
  %array_elem_13 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 13
  store i8 0, i8* %array_elem_13, align 1
  %array_elem_14 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 14
  store i8 0, i8* %array_elem_14, align 1
  %array_elem_15 = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 15
  store i8 0, i8* %array_elem_15, align 1
  %array_elem_ptr = getelementptr [16 x i8], [16 x i8]* %array_literal, i64 0, i64 0
  %array_ptr_i64 = ptrtoint i8* %array_elem_ptr to i64
  %tensor_with_ptr = insertvalue { i64, i64 } undef, i64 %array_ptr_i64, 0
  %tensor_complete = insertvalue { i64, i64 } %tensor_with_ptr, i64 16, 1
  %buf = alloca { i64, i64 }, align 8
  store { i64, i64 } %tensor_complete, { i64, i64 }* %buf, align 4
  %client_fd2 = load i64, i64* %client_fd1, align 4
  %buf3 = load { i64, i64 }, { i64, i64 }* %buf, align 4
  %tensor_ptr_cast = extractvalue { i64, i64 } %buf3, 0
  %ffi_arg_ptr_1 = inttoptr i64 %tensor_ptr_cast to i8*
  %naso.call.naso_read = call i64 @naso_read(i64 %client_fd2, i8* %ffi_arg_ptr_1, i64 5)
  %array_literal4 = alloca [51 x i8], align 1
  %array_elem_05 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 0
  store i8 72, i8* %array_elem_05, align 1
  %array_elem_16 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 1
  store i8 84, i8* %array_elem_16, align 1
  %array_elem_27 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 2
  store i8 84, i8* %array_elem_27, align 1
  %array_elem_38 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 3
  store i8 80, i8* %array_elem_38, align 1
  %array_elem_49 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 4
  store i8 47, i8* %array_elem_49, align 1
  %array_elem_510 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 5
  store i8 49, i8* %array_elem_510, align 1
  %array_elem_611 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 6
  store i8 46, i8* %array_elem_611, align 1
  %array_elem_712 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 7
  store i8 49, i8* %array_elem_712, align 1
  %array_elem_813 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 8
  store i8 32, i8* %array_elem_813, align 1
  %array_elem_914 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 9
  store i8 50, i8* %array_elem_914, align 1
  %array_elem_1015 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 10
  store i8 48, i8* %array_elem_1015, align 1
  %array_elem_1116 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 11
  store i8 48, i8* %array_elem_1116, align 1
  %array_elem_1217 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 12
  store i8 32, i8* %array_elem_1217, align 1
  %array_elem_1318 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 13
  store i8 79, i8* %array_elem_1318, align 1
  %array_elem_1419 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 14
  store i8 75, i8* %array_elem_1419, align 1
  %array_elem_1520 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 15
  store i8 13, i8* %array_elem_1520, align 1
  %array_elem_1621 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 16
  store i8 10, i8* %array_elem_1621, align 1
  %array_elem_17 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 17
  store i8 67, i8* %array_elem_17, align 1
  %array_elem_18 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 18
  store i8 111, i8* %array_elem_18, align 1
  %array_elem_19 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 19
  store i8 110, i8* %array_elem_19, align 1
  %array_elem_20 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 20
  store i8 116, i8* %array_elem_20, align 1
  %array_elem_21 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 21
  store i8 101, i8* %array_elem_21, align 1
  %array_elem_22 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 22
  store i8 110, i8* %array_elem_22, align 1
  %array_elem_23 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 23
  store i8 116, i8* %array_elem_23, align 1
  %array_elem_24 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 24
  store i8 45, i8* %array_elem_24, align 1
  %array_elem_25 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 25
  store i8 76, i8* %array_elem_25, align 1
  %array_elem_26 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 26
  store i8 101, i8* %array_elem_26, align 1
  %array_elem_2722 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 27
  store i8 110, i8* %array_elem_2722, align 1
  %array_elem_28 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 28
  store i8 103, i8* %array_elem_28, align 1
  %array_elem_29 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 29
  store i8 116, i8* %array_elem_29, align 1
  %array_elem_30 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 30
  store i8 104, i8* %array_elem_30, align 1
  %array_elem_31 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 31
  store i8 58, i8* %array_elem_31, align 1
  %array_elem_32 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 32
  store i8 32, i8* %array_elem_32, align 1
  %array_elem_33 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 33
  store i8 49, i8* %array_elem_33, align 1
  %array_elem_34 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 34
  store i8 51, i8* %array_elem_34, align 1
  %array_elem_35 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 35
  store i8 13, i8* %array_elem_35, align 1
  %array_elem_36 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 36
  store i8 10, i8* %array_elem_36, align 1
  %array_elem_37 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 37
  store i8 13, i8* %array_elem_37, align 1
  %array_elem_3823 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 38
  store i8 10, i8* %array_elem_3823, align 1
  %array_elem_39 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 39
  store i8 72, i8* %array_elem_39, align 1
  %array_elem_40 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 40
  store i8 101, i8* %array_elem_40, align 1
  %array_elem_41 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 41
  store i8 108, i8* %array_elem_41, align 1
  %array_elem_42 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 42
  store i8 108, i8* %array_elem_42, align 1
  %array_elem_43 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 43
  store i8 111, i8* %array_elem_43, align 1
  %array_elem_44 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 44
  store i8 32, i8* %array_elem_44, align 1
  %array_elem_45 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 45
  store i8 87, i8* %array_elem_45, align 1
  %array_elem_46 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 46
  store i8 111, i8* %array_elem_46, align 1
  %array_elem_47 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 47
  store i8 114, i8* %array_elem_47, align 1
  %array_elem_48 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 48
  store i8 108, i8* %array_elem_48, align 1
  %array_elem_4924 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 49
  store i8 100, i8* %array_elem_4924, align 1
  %array_elem_50 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 50
  store i8 10, i8* %array_elem_50, align 1
  %array_elem_ptr25 = getelementptr [51 x i8], [51 x i8]* %array_literal4, i64 0, i64 0
  %array_ptr_i6426 = ptrtoint i8* %array_elem_ptr25 to i64
  %tensor_with_ptr27 = insertvalue { i64, i64 } undef, i64 %array_ptr_i6426, 0
  %tensor_complete28 = insertvalue { i64, i64 } %tensor_with_ptr27, i64 51, 1
  %response = alloca { i64, i64 }, align 8
  store { i64, i64 } %tensor_complete28, { i64, i64 }* %response, align 4
  %client_fd29 = load i64, i64* %client_fd1, align 4
  %response30 = load { i64, i64 }, { i64, i64 }* %response, align 4
  %tensor_ptr_cast31 = extractvalue { i64, i64 } %response30, 0
  %ffi_arg_ptr_132 = inttoptr i64 %tensor_ptr_cast31 to i8*
  %naso.call.naso_write = call i64 @naso_write(i64 %client_fd29, i8* %ffi_arg_ptr_132, i64 57)
  %client_fd33 = load i64, i64* %client_fd1, align 4
  %naso.call.naso_close = call i64 @naso_close(i64 %client_fd33)
  ret void
}

define void @naso_accept_loop(i64 %fd) {
entry:
  %fd1 = alloca i64, align 8
  store i64 %fd, i64* %fd1, align 4
  %running = alloca i64, align 8
  store i64 1, i64* %running, align 4
  %array_literal = alloca [2 x i8], align 1
  %array_elem_0 = getelementptr [2 x i8], [2 x i8]* %array_literal, i64 0, i64 0
  store i8 0, i8* %array_elem_0, align 1
  %array_elem_1 = getelementptr [2 x i8], [2 x i8]* %array_literal, i64 0, i64 1
  store i8 0, i8* %array_elem_1, align 1
  %array_elem_ptr = getelementptr [2 x i8], [2 x i8]* %array_literal, i64 0, i64 0
  %array_ptr_i64 = ptrtoint i8* %array_elem_ptr to i64
  %tensor_with_ptr = insertvalue { i64, i64 } undef, i64 %array_ptr_i64, 0
  %tensor_complete = insertvalue { i64, i64 } %tensor_with_ptr, i64 2, 1
  %out = alloca { i64, i64 }, align 8
  store { i64, i64 } %tensor_complete, { i64, i64 }* %out, align 4
  %out2 = load { i64, i64 }, { i64, i64 }* %out, align 4
  %tensor_ptr_cast = extractvalue { i64, i64 } %out2, 0
  %out_ptr = alloca i64, align 8
  store i64 %tensor_ptr_cast, i64* %out_ptr, align 4
  %running3 = load i64, i64* %running, align 4
  %ne = icmp ne i64 %running3, 0
  %while_entry_test = icmp ne i1 %ne, false
  br i1 %while_entry_test, label %while_cond, label %while_exit

while_cond:                                       ; preds = %while_step, %entry
  %running4 = load i64, i64* %running, align 4
  %ne5 = icmp ne i64 %running4, 0
  %while_cond_val = icmp ne i1 %ne5, false
  br i1 %while_cond_val, label %while_body, label %while_exit

while_body:                                       ; preds = %while_cond
  %fd6 = load i64, i64* %fd1, align 4
  %out_ptr7 = load i64, i64* %out_ptr, align 4
  %ffi_arg_ptr_1 = inttoptr i64 %out_ptr7 to i8*
  %naso.call.naso_tcp_accept = call i64 @naso_tcp_accept(i64 %fd6, i8* %ffi_arg_ptr_1)
  %ne8 = icmp ne i64 %naso.call.naso_tcp_accept, 0
  %if_cond = icmp ne i1 %ne8, false
  br i1 %if_cond, label %then, label %else

while_exit:                                       ; preds = %while_cond, %entry
  ret void

while_step:                                       ; preds = %if_merge
  br label %while_cond

then:                                             ; preds = %while_body
  %out_ptr9 = load i64, i64* %out_ptr, align 4
  %ffi_arg_ptr_0 = inttoptr i64 %out_ptr9 to i8*
  %naso.call.naso_accept_get_client = call i64 @naso_accept_get_client(i8* %ffi_arg_ptr_0)
  %client_fd = alloca i64, align 8
  store i64 %naso.call.naso_accept_get_client, i64* %client_fd, align 4
  %client_fd10 = load i64, i64* %client_fd, align 4
  call void @naso_handle_client(i64 %client_fd10)
  br label %if_merge

else:                                             ; preds = %while_body
  br label %if_merge

if_merge:                                         ; preds = %else, %then
  %if_phi = phi i64 [ 0, %then ], [ 0, %else ]
  %naso.call.naso_yield = call i64 @naso_yield()
  br label %while_step
}

define void @naso_main() {
entry:
  %array_literal = alloca [14 x i8], align 1
  %array_elem_0 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 0
  store i8 49, i8* %array_elem_0, align 1
  %array_elem_1 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 1
  store i8 50, i8* %array_elem_1, align 1
  %array_elem_2 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 2
  store i8 55, i8* %array_elem_2, align 1
  %array_elem_3 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 3
  store i8 46, i8* %array_elem_3, align 1
  %array_elem_4 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 4
  store i8 48, i8* %array_elem_4, align 1
  %array_elem_5 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 5
  store i8 46, i8* %array_elem_5, align 1
  %array_elem_6 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 6
  store i8 48, i8* %array_elem_6, align 1
  %array_elem_7 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 7
  store i8 46, i8* %array_elem_7, align 1
  %array_elem_8 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 8
  store i8 49, i8* %array_elem_8, align 1
  %array_elem_9 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 9
  store i8 58, i8* %array_elem_9, align 1
  %array_elem_10 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 10
  store i8 56, i8* %array_elem_10, align 1
  %array_elem_11 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 11
  store i8 48, i8* %array_elem_11, align 1
  %array_elem_12 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 12
  store i8 57, i8* %array_elem_12, align 1
  %array_elem_13 = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 13
  store i8 48, i8* %array_elem_13, align 1
  %array_elem_ptr = getelementptr [14 x i8], [14 x i8]* %array_literal, i64 0, i64 0
  %array_ptr_i64 = ptrtoint i8* %array_elem_ptr to i64
  %tensor_with_ptr = insertvalue { i64, i64 } undef, i64 %array_ptr_i64, 0
  %tensor_complete = insertvalue { i64, i64 } %tensor_with_ptr, i64 14, 1
  %addr = alloca { i64, i64 }, align 8
  store { i64, i64 } %tensor_complete, { i64, i64 }* %addr, align 4
  %addr1 = load { i64, i64 }, { i64, i64 }* %addr, align 4
  %tensor_ptr_cast = extractvalue { i64, i64 } %addr1, 0
  %ffi_arg_ptr_0 = inttoptr i64 %tensor_ptr_cast to i8*
  %naso.call.naso_tcp_bind = call i64 @naso_tcp_bind(i8* %ffi_arg_ptr_0, i64 14)
  %fd = alloca i64, align 8
  store i64 %naso.call.naso_tcp_bind, i64* %fd, align 4
  %naso.call.naso_runtime_new = call i64 @naso_runtime_new(i64 4)
  %runtime = alloca i64, align 8
  store i64 %naso.call.naso_runtime_new, i64* %runtime, align 4
  %fd2 = load i64, i64* %fd, align 4
  call void @naso_accept_loop(i64 %fd2)
  %runtime3 = load i64, i64* %runtime, align 4
  %ffi_arg_ptr_04 = inttoptr i64 %runtime3 to i8*
  %naso.call.naso_runtime_run = call i64 @naso_runtime_run(i8* %ffi_arg_ptr_04)
  ret void
}

define void @naso_entry() {
entry:
  call void @naso_main()
  ret void
}
