# Mode forwarding: NAT dan DSR

Dipilih per service lewat `forwarding: nat` (default) atau `forwarding: dsr`.

## NAT

```
client ──▶ LB ──▶ backend
client ◀── LB ◀── backend
```

LB menulis ulang alamat tujuan ke backend saat masuk, lalu menulis ulang alamat sumber balik ke VIP
saat keluar. Kedua arah harus lewat LB.

Yang dibutuhkan: tidak ada. Backend tidak perlu dikonfigurasi apa pun, dan tidak tahu ada load
balancer di depannya. Ini alasan NAT jadi default.

Konsekuensinya: LB memproses seluruh trafik balasan, yang di beban HTTP nyata adalah bagian terbesar
dari byte yang mengalir. Dan LB **harus** berada di jalur balik — kalau backend punya rute langsung
ke client, balasan tidak akan pernah ter-SNAT dan koneksi gagal terbentuk.

## DSR

```
client ──▶ LB ──▶ backend
client ◀───────── backend
```

LB membungkus paket asli di dalam header IPv4 luar dengan protocol IPIP (4), lalu mengirimkannya ke
backend. Backend melepas enkapsulasi, melihat paket asli yang masih beralamat ke VIP, memprosesnya,
dan menjawab **langsung ke client** dengan source address VIP. Balasan tidak pernah menyentuh LB.

Paket asli diteruskan utuh byte per byte. Tidak ada port yang ditulis ulang, tidak ada alamat dalam
yang diubah — hanya sebuah header luar yang ditambahkan.

### Yang dibutuhkan di backend

Tiga hal, dan tanpa salah satunya paket akan dibuang tanpa jejak di sisi LB:

```
ip tunnel add ipip0 mode ipip local <backend-ip> remote <dsr_source>
ip link set ipip0 up
ip addr add <vip>/32 dev lo
sysctl -w net.ipv4.conf.all.rp_filter=0
```

VIP di loopback diperlukan supaya paket dalam yang beralamat ke VIP diterima secara lokal setelah
dekapsulasi. `rp_filter` dimatikan karena source address paket dalam adalah client, yang tidak
konsisten dengan interface tunnel tempat paket itu tiba.

### Port harus sama

DSR tidak menulis ulang port. Paket sampai ke backend dengan destination port milik service, bukan
milik backend. Config yang menyetel keduanya berbeda ditolak saat startup:

```
service web forwards with dsr to 10.0.0.11:8080 but listens on port 80; dsr does not rewrite
ports, so traffic would arrive on 80 while health checks probe 8080
```

Ini dijadikan error, bukan peringatan, karena gejalanya menyesatkan: health check hijau, tabel
maglev terisi, LB melaporkan semuanya normal, dan setiap koneksi tetap gagal.

### MTU

Enkapsulasi menambah 20 byte. Paket yang sudah sebesar MTU akan melewatinya setelah dibungkus, dan
driver akan membuangnya tanpa memberi tahu siapa pun. Deployment DSR sungguhan menangani ini dengan
menurunkan MSS atau menaikkan MTU di jaringan antara LB dan backend.

Program ini tidak melakukan pemeriksaan MTU. Menambahkan pemeriksaan berarti menanamkan asumsi angka
MTU ke dalam datapath, dan asumsi tersembunyi lebih buruk daripada batasan yang didokumentasikan.
Yang ada hanyalah counter `no_headroom` untuk kasus `bpf_xdp_adjust_head` gagal karena headroom tidak
cukup.

## Bedanya seberapa besar

Diukur di rig netns, 30 request HTTP yang sama, service yang sama, hanya mode forwarding-nya berbeda:

| | NAT | DSR | |
| --- | --- | --- | --- |
| Paket diproses LB | 412 | 238 | -42% |
| Byte diproses LB | 35.494 | 18.674 | -47% |
| Paket diteruskan LB | 360 | 210 | -42% |

DSR **menambah** 20 byte per paket arah masuk, dan totalnya tetap turun 47% karena seluruh arah
balasan hilang dari beban LB.

Catatan kejujuran: respons HTTP di rig test hanya beberapa byte. Di beban nyata, arah balasan
mendominasi — sebuah respons 50 KB terhadap request 200 byte berarti hampir seluruh byte tidak lagi
lewat LB. Angka -47% di atas adalah batas bawah, bukan batas atas.

## Bukti balasan benar-benar tidak lewat LB

Tiga pengukuran independen, bukan satu:

```
$ ip -n be1 -s link show ipip0
    RX: bytes packets ...
        5340      84                 <- tunnel dipakai arah masuk
    TX: bytes packets ...
           0       0                 <- backend tidak pernah mengenkapsulasi balik

$ curl -s localhost:9500/metrics | grep 10.0.0.11
xdplb_backend_packets_total{...} 84  <- sama persis dengan RX ipip0

$ ip netns exec lb tcpdump -ni eth0 "src 10.0.0.100"
0 packets captured                   <- tidak ada paket ber-source VIP di LB
```

## Mencobanya

```
make netns-up
make netns-dsr     # pasang tunnel ipip, VIP di lo, rute langsung ke client
sudo ip netns exec lb ./target/debug/xdp-lb --config test/config.dsr.yaml
make smoke
```

`make netns-dsr` juga memindahkan backend HTTP ke port 80, karena DSR tidak menulis ulang port.
