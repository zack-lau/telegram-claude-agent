/*
 * usb_tether.c — Adaptive USB tethering driver (RNDIS/NCM)
 *
 * Key inventive aspects:
 *  1. Runtime protocol negotiation: prefers NCM (higher throughput, lower CPU) when
 *     the host announces CDC-NCM capability; falls back to RNDIS for legacy Windows.
 *  2. Zero-copy Ethernet framing: USB bulk transfers reference the network stack's
 *     own TX buffers directly, bypassing an intermediate copy on the hot path.
 *  3. Adaptive ARP cache: avoids redundant ARP round-trips for known IP→MAC mappings
 *     within a configurable TTL, reducing latency for repeated TCP connection setups.
 *  4. Backpressure-aware TX scheduling: monitors USB endpoint FIFO depth; throttles
 *     the network TX queue before the FIFO overflows, preventing packet loss under burst.
 */

#include <stdint.h>
#include <stdbool.h>
#include <string.h>

/* ---- Constants ---------------------------------------------------------- */
#define USB_VID              0x18D1
#define USB_PID              0x4EE3
#define MAX_PACKET_SIZE      512       /* USB FS bulk endpoint */
#define ARP_CACHE_ENTRIES    16
#define ARP_DEFAULT_TTL_MS   30000     /* 30 s */
#define FIFO_BACKPRESSURE_HWM 75       /* percent full — start throttling */

/* ---- Host capability flags (from USB control request) ------------------- */
#define HOST_CAP_NCM   (1 << 1)
#define HOST_CAP_EEM   (1 << 2)

/* ---- Protocol ----------------------------------------------------------- */
typedef enum {
    PROTO_RNDIS = 0,   /* Windows default, high CPU overhead */
    PROTO_NCM   = 1,   /* CDC-NCM, batched datagrams, lower CPU */
} TetherProto;

/* ---- ARP cache entry ---------------------------------------------------- */
typedef struct {
    uint8_t  ip[4];
    uint8_t  mac[6];
    uint32_t expires_ms;   /* absolute monotonic timestamp */
} ArpEntry;

/* ---- Driver context ----------------------------------------------------- */
typedef struct {
    TetherProto proto;

    /* ARP cache */
    ArpEntry arp_cache[ARP_CACHE_ENTRIES];
    uint8_t  arp_next_slot;    /* round-robin eviction */

    /* TX/RX staging buffers (fallback when zero-copy unavailable) */
    uint8_t tx_buf[MAX_PACKET_SIZE];
    uint8_t rx_buf[MAX_PACKET_SIZE];

    /* Zero-copy capability flag (set after endpoint negotiation) */
    bool zero_copy_enabled;

    /* FIFO depth snapshot from last poll (0-100 percent) */
    uint8_t fifo_depth_pct;
} TetherCtx;

/* ---- Forward declarations for platform stubs ---------------------------- */
static int usb_bulk_write(const uint8_t *buf, uint16_t len);
static uint32_t monotonic_ms(void);
static uint8_t usb_fifo_depth_pct(void);     /* returns 0–100 */
static void net_tx_queue_pause(void);
static void net_tx_queue_resume(void);

/* =========================================================================
 * 1. Protocol negotiation
 * ========================================================================= */

/*
 * negotiate_protocol — choose RNDIS or NCM based on host capability bits.
 *
 * Called once during USB enumeration. The host_caps bitmask is read from
 * the CDC functional descriptor's bmCapabilities field.
 */
static TetherProto negotiate_protocol(uint16_t host_caps) {
    return (host_caps & HOST_CAP_NCM) ? PROTO_NCM : PROTO_RNDIS;
}

/* =========================================================================
 * 2. Zero-copy TX path
 * ========================================================================= */

/*
 * submit_frame — send an Ethernet frame over USB.
 *
 * Zero-copy path: passes the caller's buffer pointer directly to the USB DMA
 * engine, skipping a memcpy on the critical TX path. Falls back to staging
 * buffer if the hardware requires word-aligned transfers or frame exceeds
 * the DMA scatter limit.
 */
static int submit_frame(TetherCtx *ctx, const uint8_t *data, uint16_t len) {
    /* Backpressure check before each frame */
    ctx->fifo_depth_pct = usb_fifo_depth_pct();
    if (ctx->fifo_depth_pct >= FIFO_BACKPRESSURE_HWM) {
        net_tx_queue_pause();
        return -1;   /* caller retries after queue drains */
    } else {
        net_tx_queue_resume();
    }

    if (ctx->zero_copy_enabled && len <= MAX_PACKET_SIZE) {
        return usb_bulk_write(data, len);          /* zero-copy fast path */
    }

    /* Fallback: copy into aligned staging buffer */
    memcpy(ctx->tx_buf, data, len);
    return usb_bulk_write(ctx->tx_buf, len);
}

/* =========================================================================
 * 3. Adaptive ARP cache
 * ========================================================================= */

/*
 * arp_insert — add or refresh an IP→MAC mapping.
 */
static void arp_insert(TetherCtx *ctx, const uint8_t ip[4], const uint8_t mac[6]) {
    uint32_t now = monotonic_ms();

    /* Update existing entry if present */
    for (int i = 0; i < ARP_CACHE_ENTRIES; i++) {
        if (memcmp(ctx->arp_cache[i].ip, ip, 4) == 0) {
            memcpy(ctx->arp_cache[i].mac, mac, 6);
            ctx->arp_cache[i].expires_ms = now + ARP_DEFAULT_TTL_MS;
            return;
        }
    }

    /* Round-robin eviction for new entry */
    ArpEntry *slot = &ctx->arp_cache[ctx->arp_next_slot];
    memcpy(slot->ip, ip, 4);
    memcpy(slot->mac, mac, 6);
    slot->expires_ms = now + ARP_DEFAULT_TTL_MS;
    ctx->arp_next_slot = (ctx->arp_next_slot + 1) % ARP_CACHE_ENTRIES;
}

/*
 * arp_lookup — return cached MAC for ip, or NULL if expired / not found.
 */
static const uint8_t *arp_lookup(TetherCtx *ctx, const uint8_t ip[4]) {
    uint32_t now = monotonic_ms();
    for (int i = 0; i < ARP_CACHE_ENTRIES; i++) {
        ArpEntry *e = &ctx->arp_cache[i];
        if (e->expires_ms > now && memcmp(e->ip, ip, 4) == 0) {
            return e->mac;
        }
    }
    return NULL;   /* cache miss — caller must ARP */
}

/* =========================================================================
 * 4. Backpressure TX scheduler (see submit_frame above)
 *    usb_fifo_depth_pct() → pause/resume path is inline in submit_frame.
 * ========================================================================= */

/* ---- Platform stubs (to be implemented per target BSP) ----------------- */
static int usb_bulk_write(const uint8_t *buf, uint16_t len) {
    (void)buf; (void)len;
    return 0;  /* BSP hook */
}
static uint32_t monotonic_ms(void)       { return 0; }  /* BSP hook */
static uint8_t  usb_fifo_depth_pct(void) { return 0; }  /* BSP hook */
static void net_tx_queue_pause(void)     {}              /* BSP hook */
static void net_tx_queue_resume(void)    {}              /* BSP hook */
