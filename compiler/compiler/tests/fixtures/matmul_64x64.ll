; ModuleID = 'matmul_64x64'
source_filename = "matmul_64x64.naso"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

; Function: matmul_kernel
; Computes C[i][j] += A[i][k] * B[k][j] for 64x64 matrices
define void @matmul_kernel(ptr noalias %A, ptr noalias %B, ptr noalias %C, i64 %N, i64 %M, i64 %K) {
entry:
  br label %i_loop

i_loop:                                           ; preds = %k_loop_end, %entry
  %i = phi i64 [ 0, %entry ], [ %i_next, %k_loop_end ]
  %i_next = add nuw nsw i64 %i, 1
  %i_cond = icmp slt i64 %i_next, %N
  br i1 %i_cond, label %j_loop, label %exit

j_loop:                                           ; preds = %k_loop_end, %i_loop
  %j = phi i64 [ 0, %i_loop ], [ %j_next, %k_loop_end ]
  %j_next = add nuw nsw i64 %j, 1
  %j_cond = icmp slt i64 %j_next, %M
  br i1 %j_cond, label %k_loop, label %i_loop_end

i_loop_end:                                       ; preds = %j_loop
  br label %i_loop

k_loop:                                           ; preds = %j_loop
  %k = phi i64 [ 0, %j_loop ], [ %k_next, %k_loop ]
  %A_idx = getelementptr inbounds [64 x [64 x double]], ptr %A, i64 0, i64 %i, i64 %k
  %A_val = load double, ptr %A_idx, align 8
  %B_idx = getelementptr inbounds [64 x [64 x double]], ptr %B, i64 0, i64 %k, i64 %j
  %B_val = load double, ptr %B_idx, align 8
  %mul = fmul double %A_val, %B_val
  %C_idx = getelementptr inbounds [64 x [64 x double]], ptr %C, i64 0, i64 %i, i64 %j
  %C_val = load double, ptr %C_idx, align 8
  %add = fadd double %C_val, %mul
  store double %add, ptr %C_idx, align 8
  %k_next = add nuw nsw i64 %k, 1
  %k_cond = icmp slt i64 %k_next, %K
  br i1 %k_cond, label %k_loop, label %k_loop_end

k_loop_end:                                       ; preds = %k_loop
  br label %j_loop

exit:                                             ; preds = %i_loop
  ret void
}

; Main entry point
define i32 @main() {
entry:
  %A = alloca [64 x [64 x double]], align 32
  %B = alloca [64 x [64 x double]], align 32
  %C = alloca [64 x [64 x double]], align 32
  call void @matmul_kernel(ptr %A, ptr %B, ptr %C, i64 64, i64 64, i64 64)
  ret i32 0
}