; ModuleID = 'stencil_3d'
source_filename = "stencil_3d.naso"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

; Function: stencil_3d_kernel
; 3D 7-point stencil: u[i][j][k] = (u[i-1][j][k] + u[i+1][j][k] + u[i][j-1][k] + u[i][j+1][k] + u[i][j][k-1] + u[i][j][k+1] + u[i][j][k]) / 7
define void @stencil_3d_kernel(ptr noalias %u, i64 %N, i64 %M, i64 %K) {
entry:
  br label %i_loop

i_loop:                                           ; preds = %k_loop_end, %entry
  %i = phi i64 [ 1, %entry ], [ %i_next, %k_loop_end ]
  %i_next = add nuw nsw i64 %i, 1
  %i_cond = icmp slt i64 %i_next, sub i64 %N, 1
  br i1 %i_cond, label %j_loop, label %exit

j_loop:                                           ; preds = %k_loop_end, %i_loop
  %j = phi i64 [ 1, %i_loop ], [ %j_next, %k_loop_end ]
  %j_next = add nuw nsw i64 %j, 1
  %j_cond = icmp slt i64 %j_next, sub i64 %M, 1
  br i1 %j_cond, label %k_loop, label %i_loop_end

i_loop_end:                                       ; preds = %j_loop
  br label %i_loop

k_loop:                                           ; preds = %j_loop
  %k = phi i64 [ 1, %j_loop ], [ %k_next, %k_loop ]
  %k_next = add nuw nsw i64 %k, 1
  %k_cond = icmp slt i64 %k_next, sub i64 %K, 1

  ; u[i][j][k] (center)
  %center_idx = getelementptr inbounds [128 x [128 x [128 x double]]], ptr %u, i64 0, i64 %i, i64 %j, i64 %k
  %center_val = load double, ptr %center_idx, align 8

  ; u[i-1][j][k]
  %i_m1 = sub i64 %i, 1
  %idx_im1 = getelementptr inbounds [128 x [128 x [128 x double]]], ptr %u, i64 0, i64 %i_m1, i64 %j, i64 %k
  %val_im1 = load double, ptr %idx_im1, align 8

  ; u[i+1][j][k]
  %i_p1 = add i64 %i, 1
  %idx_ip1 = getelementptr inbounds [128 x [128 x [128 x double]]], ptr %u, i64 0, i64 %i_p1, i64 %j, i64 %k
  %val_ip1 = load double, ptr %idx_ip1, align 8

  ; u[i][j-1][k]
  %j_m1 = sub i64 %j, 1
  %idx_jm1 = getelementptr inbounds [128 x [128 x [128 x double]]], ptr %u, i64 0, i64 %i, i64 %j_m1, i64 %k
  %val_jm1 = load double, ptr %idx_jm1, align 8

  ; u[i][j+1][k]
  %j_p1 = add i64 %j, 1
  %idx_jp1 = getelementptr inbounds [128 x [128 x [128 x double]]], ptr %u, i64 0, i64 %i, i64 %j_p1, i64 %k
  %val_jp1 = load double, ptr %idx_jp1, align 8

  ; u[i][j][k-1]
  %k_m1 = sub i64 %k, 1
  %idx_km1 = getelementptr inbounds [128 x [128 x [128 x double]]], ptr %u, i64 0, i64 %i, i64 %j, i64 %k_m1
  %val_km1 = load double, ptr %idx_km1, align 8

  ; u[i][j][k+1]
  %k_p1 = add i64 %k, 1
  %idx_kp1 = getelementptr inbounds [128 x [128 x [128 x double]]], ptr %u, i64 0, i64 %i, i64 %j, i64 %k_p1
  %val_kp1 = load double, ptr %idx_kp1, align 8

  ; Sum all 7 values
  %sum1 = fadd double %val_im1, %val_ip1
  %sum2 = fadd double %sum1, %val_jm1
  %sum3 = fadd double %sum2, %val_jp1
  %sum4 = fadd double %sum3, %val_km1
  %sum5 = fadd double %sum4, %val_kp1
  %sum6 = fadd double %sum5, %center_val

  ; Divide by 7
  %seven = fdiv double %sum6, 7.0

  ; Store result
  store double %seven, ptr %center_idx, align 8

  br i1 %k_cond, label %k_loop, label %k_loop_end

k_loop_end:                                       ; preds = %k_loop
  br label %j_loop

exit:                                             ; preds = %i_loop
  ret void
}

; Main entry point
define i32 @main() {
entry:
  %u = alloca [128 x [128 x [128 x double]]], align 32
  call void @stencil_3d_kernel(ptr %u, i64 128, i64 128, i64 128)
  ret i32 0
}