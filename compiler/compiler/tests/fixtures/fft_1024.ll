; ModuleID = 'fft_1024'
source_filename = "fft_1024.naso"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

@twiddle_factors = constant [10 x [512 x { double, double }]] zeroinitializer, align 32

; Function: bitrev_reorder
; Bit-reverse permutation for 1024 points
define void @bitrev_reorder(ptr noalias %x, i64 %N) {
entry:
  br label %i_loop

i_loop:                                           ; preds = %i_loop, %entry
  %i = phi i64 [ 0, %entry ], [ %i_next, %i_loop ]
  %i_next = add nuw nsw i64 %i, 1
  %i_cond = icmp slt i64 %i_next, %N

  ; Compute bit-reversed index
  %bitrev = call i64 @bitrev32(i64 %i)
  %cmp = icmp sgt i64 %bitrev, %i
  br i1 %cmp, label %swap, label %no_swap

swap:                                             ; preds = %i_loop
  %src_idx = getelementptr inbounds [1024 x { double, double }], ptr %x, i64 0, i64 %i
  %dst_idx = getelementptr inbounds [1024 x { double, double }], ptr %x, i64 0, i64 %bitrev
  %src_val = load { double, double }, ptr %src_idx, align 16
  %dst_val = load { double, double }, ptr %dst_idx, align 16
  store { double, double } %src_val, ptr %dst_idx, align 16
  store { double, double } %dst_val, ptr %src_idx, align 16
  br label %no_swap

no_swap:                                          ; preds = %swap, %i_loop
  br i1 %i_cond, label %i_loop, label %exit

exit:                                             ; preds = %i_loop
  ret void
}

; Function: butterfly_stage
; Butterfly computation for one FFT stage
define void @butterfly_stage(ptr noalias %x, ptr noalias %twiddle, i64 %stage, i64 %N) {
entry:
  %stride = shl i64 1, %stage
  %half_stride = lshr i64 %stride, 1
  br label %k_loop

k_loop:                                           ; preds = %k_loop, %entry
  %k = phi i64 [ 0, %entry ], [ %k_next, %k_loop ]
  %k_next = add nuw nsw i64 %k, %stride
  %k_cond = icmp slt i64 %k_next, %N

  ; Inner loop over butterflies
  br label %j_loop

j_loop:                                           ; preds = %j_loop, %k_loop
  %j = phi i64 [ 0, %k_loop ], [ %j_next, %j_loop ]
  %j_next = add nuw nsw i64 %j, 1
  %j_cond = icmp slt i64 %j_next, %half_stride

  %idx1 = add i64 %k, %j
  %idx2 = add i64 %idx1, %half_stride

  %x1_idx = getelementptr inbounds [1024 x { double, double }], ptr %x, i64 0, i64 %idx1
  %x2_idx = getelementptr inbounds [1024 x { double, double }], ptr %x, i64 0, i64 %idx2

  %x1_val = load { double, double }, ptr %x1_idx, align 16
  %x2_val = load { double, double }, ptr %x2_idx, align 16

  %x1_real = extractvalue { double, double } %x1_val, 0
  %x1_imag = extractvalue { double, double } %x1_val, 1
  %x2_real = extractvalue { double, double } %x2_val, 0
  %x2_imag = extractvalue { double, double } %x2_val, 1

  ; Twiddle factor for this stage and position
  %tw_idx = getelementptr inbounds [10 x [512 x { double, double }]], ptr %twiddle, i64 0, i64 %stage, i64 %j
  %tw_val = load { double, double }, ptr %tw_idx, align 16
  %tw_real = extractvalue { double, double } %tw_val, 0
  %tw_imag = extractvalue { double, double } %tw_val, 1

  ; Butterfly computation:
  ; x1' = x1 + tw * x2
  ; x2' = x1 - tw * x2
  %tw_x2_real = fmul double %tw_real, %x2_real
  %tw_x2_imag = fmul double %tw_imag, %x2_imag
  %tw_x2_real2 = fmul double %tw_real, %x2_imag
  %tw_x2_imag2 = fmul double %tw_imag, %x2_real

  %prod_real = fsub double %tw_x2_real, %tw_x2_imag
  %prod_imag = fadd double %tw_x2_real2, %tw_x2_imag2

  %new_x1_real = fadd double %x1_real, %prod_real
  %new_x1_imag = fadd double %x1_imag, %prod_imag
  %new_x2_real = fsub double %x1_real, %prod_real
  %new_x2_imag = fsub double %x1_imag, %prod_imag

  %new_x1 = insertvalue { double, double } undef, double %new_x1_real, 0
  %new_x1_full = insertvalue { double, double } %new_x1, double %new_x1_imag, 1
  %new_x2 = insertvalue { double, double } undef, double %new_x2_real, 0
  %new_x2_full = insertvalue { double, double } %new_x2, double %new_x2_imag, 1

  store { double, double } %new_x1_full, ptr %x1_idx, align 16
  store { double, double } %new_x2_full, ptr %x2_idx, align 16

  br i1 %j_cond, label %j_loop, label %k_loop_end

k_loop_end:                                       ; preds = %j_loop
  br i1 %k_cond, label %k_loop, label %exit

exit:                                             ; preds = %k_loop_end
  ret void
}

; Function: fft_1024
; Full Cooley-Tukey FFT with 1024 points
define void @fft_1024(ptr noalias %x) {
entry:
  ; Bit-reverse reorder
  call void @bitrev_reorder(ptr %x, i64 1024)

  ; Butterfly stages (10 stages for 1024 = 2^10)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 0, i64 1024)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 1, i64 1024)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 2, i64 1024)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 3, i64 1024)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 4, i64 1024)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 5, i64 1024)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 6, i64 1024)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 7, i64 1024)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 8, i64 1024)
  call void @butterfly_stage(ptr %x, ptr @twiddle_factors, i64 9, i64 1024)

  ret void
}

; Helper: bitrev32 - reverse bits of 32-bit integer
declare i64 @bitrev32(i64)

; Main entry point
define i32 @main() {
entry:
  %x = alloca [1024 x { double, double }], align 32
  call void @fft_1024(ptr %x)
  ret i32 0
}