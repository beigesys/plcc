// SPDX-License-Identifier: MPL-2.0
//
// plcc program-image runtime for the Arduino Opta (docs/program-image.md).
// Flashed once; PLC programs are downloaded separately into the program slot
// (0x08180000, 512 KiB) and validated here before anything in them runs.
// Build with ../build-loader.sh. The device manifest is devices/arduino-opta.toml
// (version 2).
//
// Boot:
//   1. The Arduino core starts USB CDC (and its 1200-baud-touch thread) in
//      main(), before setup(): entering the bootloader never depends on the
//      program slot.
//   2. setup(): outputs off, Modbus up, fault handlers installed, then the
//      slot is checked (plcc_image_check: magic, format, header CRC, device,
//      versions, layout, ranges, body CRC). Only a valid image is prepared
//      (RAM window zeroed, .data copied, service table stored) and entered,
//      and only through plcc_guard_call. Holding the USER button during
//      boot leaves the program stopped.
//   3. loop(): the scan cycle of docs/process-image.md.
// A fault in the program (plcc_fault, a CPU fault, the 500 ms watchdog) puts
// the PLC in STOP: outputs off, red LED blinking, the reason on the console
// and in `info`; USB and Modbus keep running. `run` cold-starts it again.
//
// Process image map, Modbus and console as in ../runtime/runtime.ino, plus
// the commands prog, stop and run.
//
// This sketch links against the Arduino mbed_opta core and its libraries
// (LGPL-2.1 and others); nothing from them is copied here.
#include <Arduino.h>
#include <ArduinoRS485.h>
#include <ArduinoModbus.h>
#include <mbed.h>
#include <string.h>

#include "plcc_guard.h"
#include "plcc_image.h"
#include "plcc_services.h"

#define RUNTIME_DEVICE "arduino-opta"
#define RUNTIME_MANIFEST_VERSION 2
#define RUNTIME_MANIFEST_FIRST_WITH_SLOT 2
#define RUNTIME_KIND "plcc-arduino"
#define RUNTIME_ABI 1

// devices/arduino-opta.toml: [target] image and [flash.program].
constexpr uint32_t IMAGE_I = 18, IMAGE_Q = 1, IMAGE_M = 64;
static const plcc_image_expect_t expect = {
    RUNTIME_DEVICE, RUNTIME_MANIFEST_FIRST_WITH_SLOT, RUNTIME_MANIFEST_VERSION, RUNTIME_ABI,
    0x08180000u,    0x80000u,                         0x20010000u,              0x10000u,
    PLCC_SERVICE_COUNT, IMAGE_I, IMAGE_Q, IMAGE_M,
};

extern "C" const plcc_service_table_t plcc_service_table;

static const int relay_pins[4] = {RELAY1, RELAY2, RELAY3, RELAY4};
static const int relay_leds[4] = {LED_RELAY1, LED_RELAY2, LED_RELAY3, LED_RELAY4};
static const int input_pins[8] = {I1, I2, I3, I4, I5, I6, I7, I8};

enum State { ST_EMPTY, ST_RUN, ST_STOP, ST_FAULT };
static volatile State state = ST_EMPTY;
static const char *slot_why = "not checked";  // why there is no program (when ST_EMPTY)
static bool guard_ok;
static const plcc_app_t *app;  // the running program's descriptor, or null
static mbed::Ticker watchdog;

// The process image: the program's (in its RAM window) or, with no program,
// these, so Modbus and the console keep a consistent (zero) image.
static uint8_t idle_i[IMAGE_I], idle_q[IMAGE_Q], idle_m[IMAGE_M];
static uint8_t *img_i = idle_i, *img_q = idle_q, *img_m = idle_m;

constexpr unsigned HR_COUNT = IMAGE_M / 2;
constexpr unsigned IR_COUNT = IMAGE_I / 2;
constexpr unsigned COIL_COUNT = IMAGE_Q * 8;
constexpr unsigned DI_COUNT = IMAGE_I * 8;
static uint16_t hr_shadow[HR_COUNT];
static uint8_t coil_shadow[COIL_COUNT];

// ── Runtime services 0-2 (the rest of the table is libc/libm/libgcc) ──────
extern "C" {
int64_t plcc_monotonic_ns(void) {
  static uint32_t last = 0;
  static uint64_t high = 0;
  uint32_t now = micros();
  if (now < last) high += (1ULL << 32);
  last = now;
  return (int64_t)((high + now) * 1000ULL);
}

// Print a string from the program, bounded and only from the image.
void plcc_print(const char *msg) {
  char line[161];
  size_t n = 0;
  uintptr_t p = (uintptr_t)msg;
  while (n < sizeof line - 1 && (plcc_image_in_text(&expect, p + n, 1) || plcc_image_in_ram(&expect, p + n, 1)) && msg[n]) {
    line[n] = msg[n];
    n++;
  }
  line[n] = 0;
  Serial.println(line);
}

void plcc_fault(uint32_t code, const char *where) { plcc_guard_fault(code, where); }

// From any context, also a fault handler: GPIO writes only.
void plcc_guard_outputs_off(void) {
  for (int i = 0; i < 4; i++) {
    digitalWrite(relay_pins[i], LOW);
    digitalWrite(relay_leds[i], LOW);
  }
  digitalWrite(LED_USER, LOW);
}
}

// ── I/O ─────────────────────────────────────────────────────────────────
static uint16_t word_at(const uint8_t *area, unsigned n) {
  uint16_t v;
  memcpy(&v, area + 2 * n, 2);
  return v;
}

static void read_inputs() {
  uint8_t bits = 0;
  for (int i = 0; i < 8; i++)
    if (digitalRead(input_pins[i])) bits |= 1 << i;
  img_i[0] = bits;
  img_i[1] = digitalRead(BTN_USER) == LOW ? 1 : 0;
  for (unsigned n = 1; n <= 8; n++) {
    uint16_t v = analogRead(input_pins[n - 1]);
    memcpy(img_i + 2 * n, &v, 2);
  }
}

static void write_outputs() {
  uint8_t q = state == ST_RUN ? img_q[0] : 0;
  for (int i = 0; i < 4; i++) {
    digitalWrite(relay_pins[i], q >> i & 1);
    digitalWrite(relay_leds[i], q >> i & 1);
  }
  digitalWrite(LED_USER, q >> 4 & 1);
}

static void modbus_to_image() {
  for (unsigned n = 0; n < HR_COUNT; n++) {
    uint16_t v = ModbusRTUServer.holdingRegisterRead(n);
    if (v != hr_shadow[n]) memcpy(img_m + 2 * n, &v, 2);
  }
  for (unsigned n = 0; n < COIL_COUNT; n++) {
    uint8_t v = ModbusRTUServer.coilRead(n) ? 1 : 0;
    if (v != coil_shadow[n]) {
      if (v) img_q[n / 8] |= 1 << (n % 8);
      else img_q[n / 8] &= ~(1 << (n % 8));
    }
  }
}

static void image_to_modbus() {
  for (unsigned n = 0; n < HR_COUNT; n++) {
    hr_shadow[n] = word_at(img_m, n);
    ModbusRTUServer.holdingRegisterWrite(n, hr_shadow[n]);
  }
  for (unsigned n = 0; n < COIL_COUNT; n++) {
    coil_shadow[n] = img_q[n / 8] >> (n % 8) & 1;
    ModbusRTUServer.coilWrite(n, coil_shadow[n]);
  }
  for (unsigned n = 0; n < DI_COUNT; n++) ModbusRTUServer.discreteInputWrite(n, img_i[n / 8] >> (n % 8) & 1);
  for (unsigned n = 0; n < IR_COUNT; n++) ModbusRTUServer.inputRegisterWrite(n, word_at(img_i, n));
}

// ── The program ─────────────────────────────────────────────────────────
// Every call into program code is one of these, run by plcc_guard_call.
static plcc_get_app_fn get_app;
static const plcc_app_t *got_app;
static uint8_t single_value;
static void call_get_app(uint32_t) { got_app = get_app(); }
static void call_init(uint32_t) { app->init(); }
static void call_task(uint32_t t) { app->run_task(t); }
static void call_single(uint32_t t) { single_value = app->tasks[t].single(); }

static int64_t next_due[PLCC_MAX_TASKS];
static uint8_t prev_single[PLCC_MAX_TASKS];
static unsigned long fault_reported;

static void fault_line() {
  const plcc_guard_fault_t *f = plcc_guard_last();
  Serial.print("PLC STOP: fault ");
  Serial.print(f->code);
  Serial.print(f->code == PLCC_FAULT_DIV_BY_ZERO ? " (division by zero)"
               : f->code == PLCC_FAULT_ARRAY_BOUNDS ? " (array bounds)"
               : f->code == PLCC_FAULT_CPU ? " (CPU fault)"
               : f->code == PLCC_FAULT_WATCHDOG ? " (watchdog)" : "");
  Serial.print(" at ");
  Serial.print(f->where[0] ? f->where : "?");
  if (f->pc) {
    Serial.print(" pc=0x");
    Serial.print(f->pc, HEX);
  }
  Serial.println();
}

static void enter_fault() {
  state = ST_FAULT;
  memset(img_q, 0, IMAGE_Q);
  write_outputs();
  fault_reported = millis();
  fault_line();
}

static void use_idle_image() {
  app = nullptr;
  img_i = idle_i;
  img_q = idle_q;
  img_m = idle_m;
  memset(idle_q, 0, sizeof idle_q);
}

// Check the slot and cold-start the program in it. Never executes anything
// in the slot unless plcc_image_check accepted it.
static bool start_program() {
  use_idle_image();
  if (!guard_ok) {
    slot_why = "fault handlers not installed: programs are not run";
    state = ST_EMPTY;
    return false;
  }
  if (!plcc_image_check(&expect, &slot_why)) {
    state = ST_EMPTY;
    return false;
  }
  get_app = plcc_image_prepare(&expect, &plcc_service_table);
  got_app = nullptr;
  if (plcc_guard_call(call_get_app, 0)) {
    enter_fault();
    return false;
  }
  if (!plcc_image_check_app(&expect, got_app, &slot_why)) {
    state = ST_EMPTY;
    return false;
  }
  app = got_app;
  img_i = app->image->input;
  img_q = app->image->output;
  img_m = app->image->memory;
  memset(next_due, 0, sizeof next_due);
  memset(prev_single, 0, sizeof prev_single);
  state = ST_RUN;  // before init: a fault in init must find the program's image to clear
  if (plcc_guard_call(call_init, 0)) {
    enter_fault();
    return false;
  }
  slot_why = "ok";
  return true;
}

static void stop_program() {
  if (state == ST_RUN) state = ST_STOP;
  memset(img_q, 0, IMAGE_Q);
  write_outputs();
}

// The scan scheduler of docs/process-image.md (ScanCycle), as runtime.ino.
static void run_due_tasks() {
  const uint32_t count = app->task_count;
  int64_t now = plcc_monotonic_ns();
  uint8_t due[PLCC_MAX_TASKS] = {0};
  for (uint32_t t = 0; t < count; t++) {
    const plcc_task_t *task = &app->tasks[t];
    uint8_t s = 0;
    if (task->single) {
      if (plcc_guard_call(call_single, t)) {
        enter_fault();
        return;
      }
      s = single_value;
    }
    if (task->single && s && !prev_single[t]) due[t] = 1;
    prev_single[t] = s;
    if (task->interval_ns > 0 && !s && now >= next_due[t]) {
      due[t] = 1;
      next_due[t] = next_due[t] + task->interval_ns > now ? next_due[t] + task->interval_ns : now + task->interval_ns;
    }
    if (task->interval_ns == 0 && !task->single) due[t] = 1;
  }
  for (;;) {
    uint32_t best = count;
    for (uint32_t t = 0; t < count; t++)
      if (due[t] && (best == count || app->tasks[t].priority < app->tasks[best].priority)) best = t;
    if (best == count) break;
    due[best] = 0;
    if (plcc_guard_call(call_task, best)) {
      enter_fault();
      return;
    }
  }
}

// ── Console ─────────────────────────────────────────────────────────────
static void print_hex32(uint32_t v) {
  char b[9];
  for (int i = 7; i >= 0; i--, v >>= 4) b[i] = "0123456789abcdef"[v & 15];
  b[8] = 0;
  Serial.print(b);
}

static void print_build_id(const plcc_image_header_t *h) {
  for (int i = 0; i < 16; i++) {
    Serial.print("0123456789abcdef"[h->build_id[i] >> 4]);
    Serial.print("0123456789abcdef"[h->build_id[i] & 15]);
  }
}

static void print_json_string(const char *s) {
  Serial.print('"');
  for (; *s; s++) {
    if (*s == '"' || *s == '\\') Serial.print('\\');
    if ((unsigned char)*s >= 0x20) Serial.print(*s);
  }
  Serial.print('"');
}

static const char *state_name() {
  switch (state) {
    case ST_RUN: return "run";
    case ST_STOP: return "stop";
    case ST_FAULT: return "fault";
    default: return "empty";
  }
}

static const plcc_image_header_t *loaded_header() {
  return state == ST_EMPTY ? nullptr : (const plcc_image_header_t *)(uintptr_t)expect.slot_addr;
}

// info: one line of JSON (docs/device-manifest.md, "Console protocol").
static void cmd_info() {
  Serial.print("{\"device\":\"" RUNTIME_DEVICE "\",\"manifest\":");
  Serial.print(RUNTIME_MANIFEST_VERSION);
  Serial.print(",\"runtime\":\"" RUNTIME_KIND "\",\"abi\":");
  Serial.print(RUNTIME_ABI);
  Serial.print(",\"image\":{\"I\":");
  Serial.print(IMAGE_I);
  Serial.print(",\"Q\":");
  Serial.print(IMAGE_Q);
  Serial.print(",\"M\":");
  Serial.print(IMAGE_M);
  Serial.print("},\"state\":\"");
  Serial.print(state_name());
  Serial.print("\",\"program\":");
  if (const plcc_image_header_t *h = loaded_header()) {
    Serial.print("{\"build\":\"");
    print_build_id(h);
    Serial.print("\",\"size\":");
    Serial.print(h->image_size);
    Serial.print(",\"crc\":\"");
    print_hex32(h->body_crc32);
    Serial.print("\",\"version\":");
    Serial.print(h->target_version);
    Serial.print("}");
  } else {
    Serial.print("null,\"reason\":");
    print_json_string(slot_why);
  }
  if (state == ST_FAULT) {
    const plcc_guard_fault_t *f = plcc_guard_last();
    Serial.print(",\"fault\":{\"code\":");
    Serial.print(f->code);
    Serial.print(",\"where\":");
    print_json_string(f->where);
    Serial.print(",\"pc\":\"0x");
    print_hex32(f->pc);
    Serial.print("\"}");
  }
  Serial.println("}");
}

// prog: the slot's header, checked now, as one line of JSON.
static void cmd_prog() {
  const char *why = "";
  bool ok = plcc_image_check(&expect, &why);
  const plcc_image_header_t *h = (const plcc_image_header_t *)(uintptr_t)expect.slot_addr;
  Serial.print("{\"valid\":");
  Serial.print(ok ? "true" : "false");
  Serial.print(",\"why\":");
  print_json_string(why);
  if (h->magic == PLCC_IMAGE_MAGIC) {
    char id[sizeof h->target_id + 1];
    memcpy(id, h->target_id, sizeof h->target_id);
    id[sizeof h->target_id] = 0;
    Serial.print(",\"format\":");
    Serial.print(h->format);
    Serial.print(",\"target\":");
    print_json_string(id);
    Serial.print(",\"version\":");
    Serial.print(h->target_version);
    Serial.print(",\"abi\":");
    Serial.print(h->abi);
    Serial.print(",\"services\":");
    Serial.print(h->services);
    Serial.print(",\"size\":");
    Serial.print(h->image_size);
    Serial.print(",\"text\":");
    Serial.print(h->text_size);
    Serial.print(",\"data\":");
    Serial.print(h->data_size);
    Serial.print(",\"bss\":");
    Serial.print(h->bss_size);
    Serial.print(",\"build\":\"");
    print_build_id(h);
    Serial.print("\",\"header_crc\":\"");
    print_hex32(h->header_crc32);
    Serial.print("\",\"body_crc\":\"");
    print_hex32(h->body_crc32);
    Serial.print("\"");
  }
  Serial.println("}");
}

static void console() {
  static char line[40];
  static unsigned len = 0;
  while (Serial.available()) {
    char c = Serial.read();
    if (c != '\n' && c != '\r') {
      if (len < sizeof line - 1) line[len++] = c;
      continue;
    }
    line[len] = 0;
    len = 0;
    unsigned n;
    long v;
    if (sscanf(line, "mw %u %ld", &n, &v) == 2 && n < HR_COUNT) {
      uint16_t w = (uint16_t)v;
      memcpy(img_m + 2 * n, &w, 2);
      Serial.print("ok %MW");
      Serial.print(n);
      Serial.print(" := ");
      Serial.println(w);
    } else if (strcmp(line, "img") == 0) {
      Serial.print("I:");
      for (unsigned b = 0; b < IMAGE_I; b++) { Serial.print(' '); Serial.print(img_i[b], HEX); }
      Serial.print("  Q:");
      for (unsigned b = 0; b < IMAGE_Q; b++) { Serial.print(' '); Serial.print(img_q[b], HEX); }
      Serial.print("  M:");
      for (unsigned b = 0; b < IMAGE_M; b++) { Serial.print(' '); Serial.print(img_m[b], HEX); }
      Serial.println();
    } else if (strcmp(line, "info") == 0) {
      cmd_info();
    } else if (strcmp(line, "prog") == 0) {
      cmd_prog();
    } else if (strcmp(line, "stop") == 0) {
      stop_program();
      Serial.print("ok ");
      Serial.println(state_name());
    } else if (strcmp(line, "run") == 0) {
      if (start_program()) {
        Serial.println("ok run");
      } else if (state == ST_FAULT) {
        Serial.println("error: the program faulted while starting");
      } else {
        Serial.print("error: ");
        Serial.println(slot_why);
      }
    } else if (line[0]) {
      Serial.println("? commands: info | img | mw <n> <value> | prog | stop | run");
    }
  }
}

void setup() {
  // USB CDC is already up (the core's main() starts it before setup()).
  Serial.begin(115200);
  for (int i = 0; i < 4; i++) {
    pinMode(relay_pins[i], OUTPUT);
    pinMode(relay_leds[i], OUTPUT);
  }
  pinMode(LED_USER, OUTPUT);
  pinMode(LEDR, OUTPUT);
  pinMode(BTN_USER, INPUT);
  plcc_guard_outputs_off();
  digitalWrite(LEDR, LOW);
  analogReadResolution(12);

  constexpr unsigned long baudrate = 19200;
  constexpr float bit_us = 1e6f / baudrate;
  RS485.setDelays(bit_us * 9.6f * 3.5f, bit_us * 9.6f * 3.5f);
  ModbusRTUServer.begin(1, baudrate, SERIAL_8E1);
  ModbusRTUServer.configureHoldingRegisters(0, HR_COUNT);
  ModbusRTUServer.configureInputRegisters(0, IR_COUNT);
  ModbusRTUServer.configureCoils(0, COIL_COUNT);
  ModbusRTUServer.configureDiscreteInputs(0, DI_COUNT);
  image_to_modbus();

  guard_ok = plcc_guard_install(&expect);
  watchdog.attach(&plcc_guard_tick, std::chrono::milliseconds(PLCC_GUARD_TICK_MS));

  if (digitalRead(BTN_USER) == LOW) {
    // Safe start: the program is checked but not run until `run`.
    plcc_image_check(&expect, &slot_why);
    state = ST_EMPTY;
    slot_why = "USER button held at boot: program not started (send `run`)";
  } else {
    start_program();
  }
  image_to_modbus();
}

void loop() {
  ModbusRTUServer.poll();
  read_inputs();     // latch %I
  modbus_to_image();
  console();         // writes land after Modbus, before the scan
  if (state == ST_RUN) run_due_tasks();
  write_outputs();   // flush %Q (all off unless running)
  image_to_modbus();
  if (state == ST_FAULT) {
    digitalWrite(LEDR, (millis() / 250) & 1 ? HIGH : LOW);
    if (millis() - fault_reported > 4000) {
      fault_reported = millis();
      fault_line();
    }
  } else {
    digitalWrite(LEDR, LOW);
  }
}
