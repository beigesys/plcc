/* SPDX-License-Identifier: MPL-2.0
 *
 * The runtime service table of plcc program images (docs/program-image.md).
 *
 * A program image calls into the runtime only through this table: the image
 * linker (crates/plcc-image) turns every call to an undefined symbol into a
 * veneer that loads entry `index` from the table whose address the loader
 * stores at the start of the program's RAM window. Entries are APPEND-ONLY:
 * an index never changes meaning. crates/plcc-image/src/services.rs holds the
 * same list (a test there compares the two), and the device manifest's
 * `[flash.program] services` is the number of entries.
 *
 * PLCC_SERVICE(index, name): `name` is the symbol a plcc object imports.
 * Entries 0-2 are the runtime's own (runtime-symbols.md); the rest are the
 * C library and compiler helpers of the Arduino toolchain (newlib libc/libm,
 * libgcc), linked into the runtime by reference.
 */
#ifndef PLCC_SERVICES_H
#define PLCC_SERVICES_H

#define PLCC_SERVICES(PLCC_SERVICE) \
  PLCC_SERVICE(0, plcc_monotonic_ns) \
  PLCC_SERVICE(1, plcc_print) \
  PLCC_SERVICE(2, plcc_fault) \
  PLCC_SERVICE(3, memcpy) \
  PLCC_SERVICE(4, memmove) \
  PLCC_SERVICE(5, memset) \
  PLCC_SERVICE(6, memcmp) \
  PLCC_SERVICE(7, __aeabi_memcpy) \
  PLCC_SERVICE(8, __aeabi_memcpy4) \
  PLCC_SERVICE(9, __aeabi_memcpy8) \
  PLCC_SERVICE(10, __aeabi_memmove) \
  PLCC_SERVICE(11, __aeabi_memmove4) \
  PLCC_SERVICE(12, __aeabi_memmove8) \
  PLCC_SERVICE(13, __aeabi_memset) \
  PLCC_SERVICE(14, __aeabi_memset4) \
  PLCC_SERVICE(15, __aeabi_memset8) \
  PLCC_SERVICE(16, __aeabi_memclr) \
  PLCC_SERVICE(17, __aeabi_memclr4) \
  PLCC_SERVICE(18, __aeabi_memclr8) \
  PLCC_SERVICE(19, __aeabi_idiv) \
  PLCC_SERVICE(20, __aeabi_uidiv) \
  PLCC_SERVICE(21, __aeabi_idivmod) \
  PLCC_SERVICE(22, __aeabi_uidivmod) \
  PLCC_SERVICE(23, __aeabi_ldivmod) \
  PLCC_SERVICE(24, __aeabi_uldivmod) \
  PLCC_SERVICE(25, __aeabi_llsl) \
  PLCC_SERVICE(26, __aeabi_llsr) \
  PLCC_SERVICE(27, __aeabi_lasr) \
  PLCC_SERVICE(28, __aeabi_lmul) \
  PLCC_SERVICE(29, __aeabi_lcmp) \
  PLCC_SERVICE(30, __aeabi_ulcmp) \
  PLCC_SERVICE(31, __divsi3) \
  PLCC_SERVICE(32, __udivsi3) \
  PLCC_SERVICE(33, __modsi3) \
  PLCC_SERVICE(34, __umodsi3) \
  PLCC_SERVICE(35, __divdi3) \
  PLCC_SERVICE(36, __udivdi3) \
  PLCC_SERVICE(37, __moddi3) \
  PLCC_SERVICE(38, __umoddi3) \
  PLCC_SERVICE(39, __muldi3) \
  PLCC_SERVICE(40, __ashldi3) \
  PLCC_SERVICE(41, __lshrdi3) \
  PLCC_SERVICE(42, __ashrdi3) \
  PLCC_SERVICE(43, __clzsi2) \
  PLCC_SERVICE(44, __clzdi2) \
  PLCC_SERVICE(45, __ctzsi2) \
  PLCC_SERVICE(46, __ctzdi2) \
  PLCC_SERVICE(47, __popcountsi2) \
  PLCC_SERVICE(48, __popcountdi2) \
  PLCC_SERVICE(49, __aeabi_l2f) \
  PLCC_SERVICE(50, __aeabi_ul2f) \
  PLCC_SERVICE(51, __aeabi_l2d) \
  PLCC_SERVICE(52, __aeabi_ul2d) \
  PLCC_SERVICE(53, __aeabi_f2lz) \
  PLCC_SERVICE(54, __aeabi_f2ulz) \
  PLCC_SERVICE(55, __aeabi_d2lz) \
  PLCC_SERVICE(56, __aeabi_d2ulz) \
  PLCC_SERVICE(57, __floatdisf) \
  PLCC_SERVICE(58, __floatundisf) \
  PLCC_SERVICE(59, __floatdidf) \
  PLCC_SERVICE(60, __floatundidf) \
  PLCC_SERVICE(61, __fixsfdi) \
  PLCC_SERVICE(62, __fixunssfdi) \
  PLCC_SERVICE(63, __fixdfdi) \
  PLCC_SERVICE(64, __fixunsdfdi) \
  PLCC_SERVICE(65, __aeabi_fadd) \
  PLCC_SERVICE(66, __aeabi_fsub) \
  PLCC_SERVICE(67, __aeabi_frsub) \
  PLCC_SERVICE(68, __aeabi_fmul) \
  PLCC_SERVICE(69, __aeabi_fdiv) \
  PLCC_SERVICE(70, __aeabi_fcmpeq) \
  PLCC_SERVICE(71, __aeabi_fcmplt) \
  PLCC_SERVICE(72, __aeabi_fcmple) \
  PLCC_SERVICE(73, __aeabi_fcmpge) \
  PLCC_SERVICE(74, __aeabi_fcmpgt) \
  PLCC_SERVICE(75, __aeabi_fcmpun) \
  PLCC_SERVICE(76, __aeabi_dadd) \
  PLCC_SERVICE(77, __aeabi_dsub) \
  PLCC_SERVICE(78, __aeabi_drsub) \
  PLCC_SERVICE(79, __aeabi_dmul) \
  PLCC_SERVICE(80, __aeabi_ddiv) \
  PLCC_SERVICE(81, __aeabi_dcmpeq) \
  PLCC_SERVICE(82, __aeabi_dcmplt) \
  PLCC_SERVICE(83, __aeabi_dcmple) \
  PLCC_SERVICE(84, __aeabi_dcmpge) \
  PLCC_SERVICE(85, __aeabi_dcmpgt) \
  PLCC_SERVICE(86, __aeabi_dcmpun) \
  PLCC_SERVICE(87, __aeabi_f2d) \
  PLCC_SERVICE(88, __aeabi_d2f) \
  PLCC_SERVICE(89, __aeabi_i2f) \
  PLCC_SERVICE(90, __aeabi_ui2f) \
  PLCC_SERVICE(91, __aeabi_i2d) \
  PLCC_SERVICE(92, __aeabi_ui2d) \
  PLCC_SERVICE(93, __aeabi_f2iz) \
  PLCC_SERVICE(94, __aeabi_f2uiz) \
  PLCC_SERVICE(95, __aeabi_d2iz) \
  PLCC_SERVICE(96, __aeabi_d2uiz) \
  PLCC_SERVICE(97, __addsf3) \
  PLCC_SERVICE(98, __subsf3) \
  PLCC_SERVICE(99, __mulsf3) \
  PLCC_SERVICE(100, __divsf3) \
  PLCC_SERVICE(101, __adddf3) \
  PLCC_SERVICE(102, __subdf3) \
  PLCC_SERVICE(103, __muldf3) \
  PLCC_SERVICE(104, __divdf3) \
  PLCC_SERVICE(105, __eqsf2) \
  PLCC_SERVICE(106, __nesf2) \
  PLCC_SERVICE(107, __ltsf2) \
  PLCC_SERVICE(108, __lesf2) \
  PLCC_SERVICE(109, __gtsf2) \
  PLCC_SERVICE(110, __gesf2) \
  PLCC_SERVICE(111, __unordsf2) \
  PLCC_SERVICE(112, __eqdf2) \
  PLCC_SERVICE(113, __nedf2) \
  PLCC_SERVICE(114, __ltdf2) \
  PLCC_SERVICE(115, __ledf2) \
  PLCC_SERVICE(116, __gtdf2) \
  PLCC_SERVICE(117, __gedf2) \
  PLCC_SERVICE(118, __unorddf2) \
  PLCC_SERVICE(119, __extendsfdf2) \
  PLCC_SERVICE(120, __truncdfsf2) \
  PLCC_SERVICE(121, __floatsisf) \
  PLCC_SERVICE(122, __floatunsisf) \
  PLCC_SERVICE(123, __floatsidf) \
  PLCC_SERVICE(124, __floatunsidf) \
  PLCC_SERVICE(125, __fixsfsi) \
  PLCC_SERVICE(126, __fixunssfsi) \
  PLCC_SERVICE(127, __fixdfsi) \
  PLCC_SERVICE(128, __fixunsdfsi) \
  PLCC_SERVICE(129, sin) \
  PLCC_SERVICE(130, cos) \
  PLCC_SERVICE(131, tan) \
  PLCC_SERVICE(132, asin) \
  PLCC_SERVICE(133, acos) \
  PLCC_SERVICE(134, atan) \
  PLCC_SERVICE(135, atan2) \
  PLCC_SERVICE(136, sinh) \
  PLCC_SERVICE(137, cosh) \
  PLCC_SERVICE(138, tanh) \
  PLCC_SERVICE(139, exp) \
  PLCC_SERVICE(140, exp2) \
  PLCC_SERVICE(141, log) \
  PLCC_SERVICE(142, log2) \
  PLCC_SERVICE(143, log10) \
  PLCC_SERVICE(144, pow) \
  PLCC_SERVICE(145, sqrt) \
  PLCC_SERVICE(146, fmod) \
  PLCC_SERVICE(147, round) \
  PLCC_SERVICE(148, floor) \
  PLCC_SERVICE(149, ceil) \
  PLCC_SERVICE(150, trunc) \
  PLCC_SERVICE(151, fabs) \
  PLCC_SERVICE(152, fmin) \
  PLCC_SERVICE(153, fmax) \
  PLCC_SERVICE(154, ldexp) \
  PLCC_SERVICE(155, rint) \
  PLCC_SERVICE(156, nearbyint) \
  PLCC_SERVICE(157, sinf) \
  PLCC_SERVICE(158, cosf) \
  PLCC_SERVICE(159, tanf) \
  PLCC_SERVICE(160, asinf) \
  PLCC_SERVICE(161, acosf) \
  PLCC_SERVICE(162, atanf) \
  PLCC_SERVICE(163, atan2f) \
  PLCC_SERVICE(164, sinhf) \
  PLCC_SERVICE(165, coshf) \
  PLCC_SERVICE(166, tanhf) \
  PLCC_SERVICE(167, expf) \
  PLCC_SERVICE(168, exp2f) \
  PLCC_SERVICE(169, logf) \
  PLCC_SERVICE(170, log2f) \
  PLCC_SERVICE(171, log10f) \
  PLCC_SERVICE(172, powf) \
  PLCC_SERVICE(173, sqrtf) \
  PLCC_SERVICE(174, fmodf) \
  PLCC_SERVICE(175, roundf) \
  PLCC_SERVICE(176, floorf) \
  PLCC_SERVICE(177, ceilf) \
  PLCC_SERVICE(178, truncf) \
  PLCC_SERVICE(179, fabsf) \
  PLCC_SERVICE(180, fminf) \
  PLCC_SERVICE(181, fmaxf) \
  PLCC_SERVICE(182, ldexpf) \
  PLCC_SERVICE(183, rintf) \
  PLCC_SERVICE(184, nearbyintf)

/* Number of entries: the manifest's [flash.program] services. */
#define PLCC_SERVICE_COUNT 185u

#endif
