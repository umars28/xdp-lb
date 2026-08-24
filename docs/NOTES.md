# Catatan implementasi

Hal-hal yang menghabiskan waktu paling banyak, dan penjelasannya.

## `include_bytes!` menghasilkan byte yang alignment-nya 1

Gejalanya: object BPF hasil compile ditolak loader dengan pesan yang tidak informatif.

```
Error: loading the BPF object into the kernel
Caused by:
    0: error parsing BPF object: error parsing ELF data
    1: error parsing ELF data
```

Yang menyesatkan: `bpftool prog load` menerima object yang sama tanpa keluhan, dan `readelf -S`
menunjukkan semua section yang seharusnya ada — `xdp`, `.maps`, `.BTF`. Jadi object-nya memang benar.

Penyebab sebenarnya baru kelihatan setelah error-nya dicetak dengan `{:?}` alih-alih `{}`:

```
ElfError(Error("Invalid ELF header size or alignment"))
```

Parser ELF yang dipakai loader membaca header lewat cast zero-copy, yang menuntut slice-nya
ter-align 8 byte. `include_bytes!` mengembalikan `&'static [u8; N]` — dan `[u8; N]` punya alignment
1. Di mana byte itu mendarat di segmen `.rodata` murni kebetulan, jadi bug ini bisa muncul atau tidak
tergantung hal-hal yang sama sekali tidak berhubungan, seperti bertambahnya satu string literal di
tempat lain.

Solusinya membungkus array di dalam struct yang alignment-nya dipaksa:

```rust
#[repr(align(8))]
struct Aligned<T: ?Sized>(T);

static OBJECT: &Aligned<[u8]> = &Aligned(*include_bytes!(env!("BPF_OBJECT")));
```

Ada test yang menjaga ini supaya tidak diam-diam rusak lagi, dan test itu tidak butuh root karena
hanya mem-parse object tanpa menyentuh kernel:

```
cargo test --test object
```

Pelajaran yang lebih umum: kalau library membungkus error dengan `thiserror`, `{}` hanya mencetak
lapisan terluar. `{:?}` mencetak seluruh rantai. Satu perubahan format itu memotong waktu diagnosis
dari jam ke menit.

## `-fno-addrsig` dan strip DWARF: benar, tapi bukan penyebabnya

Sebelum penyebab alignment ketemu, tersangka pertama adalah section `.llvm_addrsig` bertipe
`LOOS+0xfff4c03` — tipe non-standar yang wajar dicurigai bikin parser bingung. Section itu dihapus,
DWARF juga di-strip, dan error-nya **tetap sama**. Jadi hipotesis itu salah.

Kedua langkah tetap dipertahankan, tapi dengan alasan yang jujur: object jadi turun dari sekitar
40 KB ke 22 KB, dan DWARF tidak dipakai sama sekali saat runtime. `.BTF` sengaja tidak ikut
di-strip — loader membacanya untuk mengetahui tipe key dan value setiap map.

## `bpf_spin_lock` tidak diizinkan di map LRU

Rate limiter awalnya memakai `bpf_spin_lock` di dalam value map supaya akunting token-nya presisi
walau beberapa CPU menyentuh bucket yang sama. Kernel menolak saat pembuatan map:

```
MapError(CreateError { name: "rate_buckets", code: -1,
         io_error: Os { code: 95, kind: Unsupported, message: "Operation not supported" } })
```

`map_check_btf` di kernel hanya mengizinkan spin lock di `BPF_MAP_TYPE_HASH`, `BPF_MAP_TYPE_ARRAY`,
dan beberapa storage map. LRU tidak termasuk, dan alasannya masuk akal: eviction LRU bisa
membebaskan sebuah elemen sementara ada yang memegang lock di dalamnya.

Pindah ke `BPF_MAP_TYPE_HASH` biasa akan membuat verifier senang, tapi salah secara operasional. Map
akan bertambah satu entry per alamat sumber dan tidak pernah menyusut — sebuah rate limiter yang
justru bisa dihabiskan memorinya oleh penyerang yang mengganti-ganti source IP. Yang dipertahankan
justru harus eviction-nya.

Yang dipakai akhirnya `BPF_MAP_TYPE_LRU_PERCPU_HASH`. Tiap CPU punya salinan bucket sendiri, jadi
tidak ada dua CPU yang menulis ke lokasi yang sama dan lock tidak dibutuhkan sama sekali. Bonusnya
justru sejalan dengan alasan memakai XDP: tidak ada cache line yang diperebutkan di jalur panas.

Harganya nyata dan harus disebut: batasnya jadi per-CPU. Control plane membagi rate dan burst dengan
jumlah CPU, sehingga agregatnya mendekati angka yang dikonfigurasi **kalau** trafik menyebar rata
antar CPU. Kalau menumpuk di satu CPU — misalnya semua dari satu flow hash — batasnya jadi lebih
ketat dari yang diminta.

Untuk mekanisme pertahanan, menyimpang ke arah lebih ketat adalah arah yang benar. Kalau yang
dibutuhkan adalah akunting yang presisi, XDP bukan tempatnya.

## Client harus berada di subnet berbeda dari backend

Di rig test, semua namespace tergabung ke satu bridge, jadi secara L2 mereka satu segmen. Tapi client
diberi alamat `10.1.0.10/24` sementara backend `10.0.0.0/24`.

Ini bukan kosmetik. Kalau client ikut `10.0.0.0/24`, backend akan melihatnya sebagai tetangga satu
subnet, menjawab langsung lewat ARP, dan paket balasan tidak pernah lewat load balancer. Reverse NAT
tidak jalan, client menerima paket dengan source IP backend padahal koneksinya ke VIP, dan TCP
handshake gagal.

Dengan client di subnet lain, backend terpaksa memakai default route ke LB, dan LB dapat kesempatan
mengembalikan source IP ke VIP.

Konsekuensi yang sama berlaku di produksi: mode NAT menuntut load balancer berada di jalur balik.
Kalau tidak bisa dijamin, yang dibutuhkan adalah DSR, bukan NAT.

## Verdict counter tidak menjumlah persis ke `rx`

Dari satu sesi test:

```
xdplb_packets_total{verdict="rx"}              527
xdplb_packets_total{verdict="tx"}              480
xdplb_packets_total{verdict="pass"}             46
xdplb_packets_total{verdict="drop"}              0
```

480 + 46 = 526, bukan 527. Selisihnya bukan bug: paket non-IPv4 seperti ARP keluar lewat `XDP_PASS`
di awal fungsi sebelum counter `pass` sempat dinaikkan. Counter `pass` sengaja hanya mencatat paket
yang sudah lolos parsing tapi tidak cocok dengan service manapun, karena itulah angka yang berguna
saat men-debug konfigurasi VIP.

## `pkill -f` cocok dengan shell-nya sendiri

Berkali-kali `sudo pkill -f "debug/xdp-lb"` membunuh shell yang menjalankannya, karena pola itu ada
di cmdline shell tersebut. Akibatnya perintah setelahnya tidak pernah jalan, dan log yang terbaca
adalah sisa run sebelumnya — yang sempat membuat bug yang sudah diperbaiki terlihat masih ada.

Karena itu operasi start/stop backend dipindah ke `test/backend-up.sh` dan `test/backend-down.sh`,
yang memakai pola bracket `[b]ackend.py` supaya tidak pernah cocok dengan dirinya sendiri.
