(* Arduino Opta — scan-counter blink.
   No timers, so this compiles with zero undefined symbols: the whole program
   is self-contained ST with nothing to link against. `led0` is declared first
   so the firmware can read it at offset 0 of the state struct. *)

PROGRAM OptaBlink
VAR
    led0    : BOOL  := FALSE;
    counter : UDINT := 0;
END_VAR
    (* Scan-counted, not timed, so the rate depends on whatever core clock the
       bootloader left configured. This count stays visibly slow at either the
       64 MHz HSI the H7 boots on or the ~480 MHz PLL the bootloader may set. *)
    counter := counter + 1;
    IF counter >= 4000000 THEN
        counter := 0;
        led0 := NOT led0;
    END_IF;
END_PROGRAM
