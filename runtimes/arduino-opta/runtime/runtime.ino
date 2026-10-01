// SPDX-License-Identifier: MPL-2.0
//
// Generic plcc runtime for the Arduino Opta, with the program LINKED IN: runs
// any plcc-compiled ST program; nothing here depends on the program. Build
// with ../build.sh <program.st>. (The program-image runtime, which loads
// programs downloaded separately, is ../loader; see ../README.md.)
//
// This sketch links against the Arduino mbed_opta core and its libraries
// (LGPL-2.1 and others); nothing from them is copied here.
//
// Process image map (fixed for this runtime):
//   %IX0.0-0.7  I1..I8 as digital inputs
//   %IX1.0      USER button (TRUE while pressed)
//   %IW1..%IW8  I1..I8 as analog, raw 0..4095 (12-bit, 0..10 V)
//   %QX0.0-0.3  relays 1..4 (and their LEDs)
//   %QX0.4      blue USER LED
//
// Modbus RTU server on RS485: unit 1, 19200 8E1
//   coils n            <-> %QX(n/8).(n%8)
//   discrete inputs n   -> %IX(n/8).(n%8)
//   input registers n   -> %IWn
//   holding registers n <-> %MWn
// A holding register or coil written by the master is applied to the image
// before the next scan; after the scan the image is published back.
//
// This runtime is what the plcc device manifest devices/arduino-opta.toml
// describes (docs/device-manifest.md in the plcc repository); `info` on the
// USB console reports which manifest version it implements.
#include <Arduino.h>
#include <ArduinoRS485.h>
#include <ArduinoModbus.h>
#include <string.h>
#include "plc.h"

#define RUNTIME_DEVICE "arduino-opta"
#define RUNTIME_MANIFEST_VERSION 1
#define RUNTIME_KIND "plcc-arduino"
#define RUNTIME_ABI 1

static_assert(PLCC_ABI_VERSION == RUNTIME_ABI, "plc.h was generated for another runtime ABI");
#ifdef PLCC_DEVICE_ID
constexpr bool same_str(const char *a, const char *b) {
  return *a == *b && (*a == 0 || same_str(a + 1, b + 1));
}
static_assert(same_str(PLCC_DEVICE_ID, RUNTIME_DEVICE),
              "the program was compiled for another device (plcc compile --device " PLCC_DEVICE_ID ")");
#endif

#define RELAY_PIN(i) ((int[]){RELAY1, RELAY2, RELAY3, RELAY4}[i])
#define RELAY_LED(i) ((int[]){LED_RELAY1, LED_RELAY2, LED_RELAY3, LED_RELAY4}[i])

extern "C" {
int64_t plcc_monotonic_ns(void) {
  static uint32_t last = 0;
  static uint64_t high = 0;
  uint32_t now = micros();
  if (now < last) high += (1ULL << 32);
  last = now;
  return (int64_t)((high + now) * 1000ULL);
}
void plcc_print(const char *msg) { Serial.println(msg); }

// A runtime fault (e.g. integer division by zero) puts the PLC in STOP, as a
// Logix/CODESYS controller would: outputs off, tasks halted, red LED blinking,
// the fault reported on USB serial every few seconds. Only a reset restarts it.
void plcc_fault(uint32_t code, const char *where) {
  if (PLCC_IMAGE_Q_SIZE) memset(plcc_image_q, 0, PLCC_IMAGE_Q_SIZE);
  for (int i = 0; i < 4; i++) {
    digitalWrite(RELAY_PIN(i), LOW);
    digitalWrite(RELAY_LED(i), LOW);
  }
  digitalWrite(LED_USER, LOW);
  pinMode(LEDR, OUTPUT);
  for (unsigned n = 0;; n++) {
    digitalWrite(LEDR, n & 1 ? HIGH : LOW);
    if (n % 16 == 0) {
      Serial.print("PLC STOP: fault ");
      Serial.print(code);
      Serial.print(code == PLCC_FAULT_DIV_BY_ZERO ? " (division by zero)" : "");
      Serial.print(" at ");
      Serial.println(where ? where : "?");
    }
    delay(250);
  }
}
}

static const int relay_pins[4] = {RELAY1, RELAY2, RELAY3, RELAY4};
static const int relay_leds[4] = {LED_RELAY1, LED_RELAY2, LED_RELAY3, LED_RELAY4};
static const int input_pins[8] = {I1, I2, I3, I4, I5, I6, I7, I8};

// Image sizes may be 0 when a program uses no address in an area.
static uint8_t *const img_i = PLCC_IMAGE_I_SIZE ? plcc_image_i : nullptr;
static uint8_t *const img_q = PLCC_IMAGE_Q_SIZE ? plcc_image_q : nullptr;
static uint8_t *const img_m = PLCC_IMAGE_M_SIZE ? plcc_image_m : nullptr;

constexpr unsigned HR_COUNT = PLCC_IMAGE_M_SIZE / 2;
constexpr unsigned IR_COUNT = PLCC_IMAGE_I_SIZE / 2;
constexpr unsigned COIL_COUNT = PLCC_IMAGE_Q_SIZE * 8;
constexpr unsigned DI_COUNT = PLCC_IMAGE_I_SIZE * 8;

static uint16_t hr_shadow[HR_COUNT ? HR_COUNT : 1];
static uint8_t coil_shadow[COIL_COUNT ? COIL_COUNT : 1];

static uint16_t word_at(const uint8_t *area, unsigned n) {
  uint16_t v;
  memcpy(&v, area + 2 * n, 2);
  return v;
}

static void read_inputs() {
  if (!img_i) return;
  uint8_t bits = 0;
  for (int i = 0; i < 8; i++)
    if (digitalRead(input_pins[i])) bits |= 1 << i;
  img_i[0] = bits;
  if (PLCC_IMAGE_I_SIZE > 1) img_i[1] = digitalRead(BTN_USER) == LOW ? 1 : 0;
  for (unsigned n = 1; n <= 8 && 2 * n + 1 < PLCC_IMAGE_I_SIZE; n++) {
    uint16_t v = analogRead(input_pins[n - 1]);
    memcpy(img_i + 2 * n, &v, 2);
  }
}

static void write_outputs() {
  uint8_t q = img_q ? img_q[0] : 0;
  for (int i = 0; i < 4; i++) {
    digitalWrite(relay_pins[i], q >> i & 1);
    digitalWrite(relay_leds[i], q >> i & 1);
  }
  digitalWrite(LED_USER, q >> 4 & 1);
}

// Master writes since the last scan go into the image.
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
  for (unsigned n = 0; n < DI_COUNT; n++)
    ModbusRTUServer.discreteInputWrite(n, img_i[n / 8] >> (n % 8) & 1);
  for (unsigned n = 0; n < IR_COUNT; n++)
    ModbusRTUServer.inputRegisterWrite(n, word_at(img_i, n));
}

#if PLCC_TASK_COUNT > 0
static int64_t next_due[PLCC_TASK_COUNT];
static uint8_t prev_single[PLCC_TASK_COUNT];

static void run_due_tasks() {
  int64_t now = plcc_monotonic_ns();
  uint8_t due[PLCC_TASK_COUNT] = {0};
  for (uint32_t t = 0; t < PLCC_TASK_COUNT; t++) {
    const plcc_task_t *task = &plcc_tasks[t];
    uint8_t s = task->single ? task->single() : 0;
    if (task->single && s && !prev_single[t]) due[t] = 1;
    prev_single[t] = s;
    if (task->interval_ns > 0 && !s && now >= next_due[t]) {
      due[t] = 1;
      // Skip missed periods instead of bursting to catch up.
      next_due[t] = next_due[t] + task->interval_ns > now ? next_due[t] + task->interval_ns
                                                          : now + task->interval_ns;
    }
    if (task->interval_ns == 0 && !task->single) due[t] = 1;
  }
  for (;;) {
    uint32_t best = PLCC_TASK_COUNT;
    for (uint32_t t = 0; t < PLCC_TASK_COUNT; t++)
      if (due[t] && (best == PLCC_TASK_COUNT || plcc_tasks[t].priority < plcc_tasks[best].priority))
        best = t;
    if (best == PLCC_TASK_COUNT) break;
    due[best] = 0;
    plcc_run_task(best);
  }
}
#else
static void run_due_tasks() {}
#endif

// USB serial console (the manifest's [console]; docs/device-manifest.md):
//   info             one line of JSON: device id, manifest version, runtime, ABI, image sizes
//   mw <n> <value>   write %MWn (the same memory as holding register n)
//   img              print the %I, %Q and %M bytes (all of them) in unpadded hex
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
      Serial.print("ok %MW"); Serial.print(n); Serial.print(" := "); Serial.println(w);
    } else if (strcmp(line, "img") == 0) {
      Serial.print("I:");
      for (unsigned b = 0; b < PLCC_IMAGE_I_SIZE; b++) { Serial.print(' '); Serial.print(img_i[b], HEX); }
      Serial.print("  Q:");
      for (unsigned b = 0; b < PLCC_IMAGE_Q_SIZE; b++) { Serial.print(' '); Serial.print(img_q[b], HEX); }
      Serial.print("  M:");
      for (unsigned b = 0; b < PLCC_IMAGE_M_SIZE; b++) { Serial.print(' '); Serial.print(img_m[b], HEX); }
      Serial.println();
    } else if (strcmp(line, "info") == 0) {
      Serial.print("{\"device\":\"" RUNTIME_DEVICE "\",\"manifest\":");
      Serial.print(RUNTIME_MANIFEST_VERSION);
      Serial.print(",\"runtime\":\"" RUNTIME_KIND "\",\"abi\":");
      Serial.print(RUNTIME_ABI);
      Serial.print(",\"image\":{\"I\":");
      Serial.print(PLCC_IMAGE_I_SIZE);
      Serial.print(",\"Q\":");
      Serial.print(PLCC_IMAGE_Q_SIZE);
      Serial.print(",\"M\":");
      Serial.print(PLCC_IMAGE_M_SIZE);
      Serial.println("}}");
    } else if (line[0]) {
      Serial.println("? commands: info | img | mw <n> <value>");
    }
  }
}

void setup() {
  Serial.begin(115200);
  analogReadResolution(12);
  for (int i = 0; i < 4; i++) {
    pinMode(relay_pins[i], OUTPUT);
    pinMode(relay_leds[i], OUTPUT);
  }
  pinMode(LED_USER, OUTPUT);
  pinMode(BTN_USER, INPUT);

  plcc_init();  // cold start; RETAIN persistence is not wired on this board yet

  constexpr unsigned long baudrate = 19200;
  constexpr float bit_us = 1e6f / baudrate;
  RS485.setDelays(bit_us * 9.6f * 3.5f, bit_us * 9.6f * 3.5f);
  ModbusRTUServer.begin(1, baudrate, SERIAL_8E1);
  if (HR_COUNT) ModbusRTUServer.configureHoldingRegisters(0, HR_COUNT);
  if (IR_COUNT) ModbusRTUServer.configureInputRegisters(0, IR_COUNT);
  if (COIL_COUNT) ModbusRTUServer.configureCoils(0, COIL_COUNT);
  if (DI_COUNT) ModbusRTUServer.configureDiscreteInputs(0, DI_COUNT);
  image_to_modbus();
}

void loop() {
  ModbusRTUServer.poll();
  read_inputs();          // latch %I
  modbus_to_image();
  console();              // writes land after Modbus, before the scan
  run_due_tasks();
  write_outputs();        // flush %Q
  image_to_modbus();
}
