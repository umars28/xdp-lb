# Development

## Kenapa butuh VM Linux

XDP adalah hook di kernel Linux. Tidak ada padanannya di macOS — bukan soal library yang belum
di-port, tapi memang tidak ada tempat untuk attach. Apple clang juga tidak punya BPF backend, jadi
`clang -target bpf` gagal sebelum sampai ke masalah kernel.

```
$ clang -target bpf -c bpf/xdp_lb.c
error: unable to create target: 'No available targets are compatible with triple "bpf"'
```

## Menyiapkan VM

```
make vm       # limactl start --name=xdp-lb lima/xdp-lb.yaml
make shell    # masuk ke VM, cwd sudah di direktori project
```

Direktori project di-mount ke VM di path yang sama seperti di host, jadi edit di macOS, compile dan
jalankan di VM. `target/` ikut ke-share — kalau kamu juga pernah `cargo build` di host, hapus
`target/` sekali supaya artifact host dan VM tidak bercampur.

## Build

```
make build
```

`build.rs` memanggil clang untuk compile `bpf/xdp_lb.c` menjadi ELF object, lalu object itu
di-`include_bytes!` ke dalam binary Rust. Hasilnya satu binary tanpa file `.o` terpisah yang harus
ikut di-deploy.

Flag `-g` wajib: definisi map di `SEC(".maps")` disimpan sebagai BTF, dan aya membaca BTF itu untuk
tahu tipe key/value tiap map. Tanpa `-g`, load gagal dengan pesan yang tidak jelas.

## Menjalankan rig test

Topologi test tidak memakai NIC fisik. Semua node adalah network namespace yang tergabung ke satu
bridge di root namespace:

```
                      ┌──────────────── br-xdplb (root ns) ────────────────┐
                      │                                                    │
              ┌───────┴───────┐   ┌──────────────┐   ┌──────────────┐  ┌───┴──────────┐
              │ ns client     │   │ ns lb        │   │ ns be1       │  │ ns be2       │
              │ 10.1.0.10/24  │   │ 10.0.0.1/24  │   │ 10.0.0.11/24 │  │ 10.0.0.12/24 │
              │               │   │ xdp attached │   │ :8080        │  │ :8080        │
              └───────────────┘   └──────────────┘   └──────────────┘  └──────────────┘
```

Client sengaja ditaruh di subnet berbeda (`10.1.0.0/24`) walaupun secara L2 satu bridge. Ini bukan
detail kosmetik: kalau client ada di `10.0.0.0/24`, backend akan menjawab langsung ke client karena
melihatnya sebagai tetangga satu subnet, dan paket balasan tidak pernah lewat load balancer. Akibatnya
reverse NAT tidak jalan dan client menerima paket dengan source IP backend, bukan VIP — TCP handshake
gagal. Dengan client di subnet lain, backend terpaksa memakai default route ke LB.

```
make netns-up      # bikin topologi + jalankan dua backend HTTP
make run           # attach xdp di ns lb, control plane jalan di foreground
make smoke         # 40 request dari ns client ke VIP, hitung distribusinya
make netns-down
```

## Debugging

Urutan yang biasanya paling cepat menemukan masalah:

```
ip netns exec lb bpftool net show                 # program benar-benar ter-attach?
curl -s localhost:9500/metrics | grep xdplb_      # paket masuk? verdict-nya apa?
ip netns exec lb bpftool map dump name conntrack  # entry NAT terbentuk?
ip netns exec client tcpdump -ni eth0 -c 20       # paket keluar dengan dst VIP?
ip netns exec be1 tcpdump -ni eth0 -c 20          # paket sampai ke backend?
```

Beberapa verdict dan artinya:

| Metric | Arti |
| --- | --- |
| `xdplb_packets_total{verdict="rx"}` naik, sisanya diam | paket masuk tapi ditolak saat parsing — cek `iph->ihl != 5` (paket ber-IP options) |
| `verdict="pass"` naik | destinasi tidak match service manapun; cek VIP dan port di config |
| `verdict="no_backend"` naik | maglev table kosong atau backend inactive; cek `xdplb_backend_up` dan neighbour table |
| `verdict="conntrack_miss"` naik terus tanpa `conntrack_hit` | entry tidak terbentuk atau key arah balik tidak match; ini biasanya bug byte order |

## Kenapa SKB mode default

`--xdp-mode` default-nya `skb`, artinya program jalan di generic XDP — setelah `sk_buff`
dialokasikan. Ini membuang keuntungan performa utama XDP, tapi jalan di mana saja termasuk veth di
dalam VM. Untuk benchmark, pakai `--xdp-mode driver` di NIC fisik yang punya XDP native support.
Angka dari SKB mode tidak boleh dilaporkan sebagai angka XDP.
