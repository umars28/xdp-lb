# Bobot dinamis

Tanpa blok `weighting`, bobot backend diambil dari config dan tidak pernah berubah. Dengan blok itu,
control plane menanyakan sebuah query PromQL ke Prometheus atau Mimir tiap interval, lalu menghitung
ulang bobot maglev dari hasilnya.

Datapath tidak ikut berubah sama sekali. Yang berubah hanya isi tabel maglev di BPF map — jadi biaya
per paket tetap satu lookup array, seberapa rumit pun logika bobotnya di user space.

## Konfigurasi

```yaml
weighting:
  endpoint: "http://mimir.internal:9090"
  query: |
    100 - (avg by (instance) (rate(node_cpu_seconds_total{mode="idle"}[1m])) * 100)
  mode: proportional
  instance_label: instance
  interval_secs: 15
  timeout_ms: 2000
  min_weight: 1
  max_weight: 16
```

## Dua mode, dan cara memilihnya

**`proportional`** — nilai query dibaca sebagai **kapasitas**. Makin besar, makin banyak trafik.
Pakai ini kalau query-mu sudah menghasilkan "sisa kemampuan", misalnya `100 - cpu_utilisation` atau
jumlah worker yang idle.

**`inverse`** — nilai query dibaca sebagai **biaya**. Makin besar, makin sedikit trafik. Pakai ini
kalau query-mu menghasilkan latensi atau panjang antrean. Backend dengan p99 dua kali lipat akan
dapat setengah bobot.

Bobot dinormalisasi terhadap backend terkuat: yang tertinggi selalu dapat `max_weight`, sisanya
proporsional, lalu di-clamp ke `min_weight`. Normalisasi ini penting karena skala metrik tidak bisa
diasumsikan — query yang mengembalikan 0–100 dan query yang mengembalikan 0–1 sama-sama bekerja.

## Mencocokkan seri metrik dengan backend

Label yang dibaca ditentukan `instance_label` (default `instance`). Port exporter dibuang, jadi
`10.0.0.11:9100` cocok dengan backend `10.0.0.11:8080`.

Kalau backend-mu tidak dikenali Prometheus lewat IP-nya, isi `metrics_instance` di backend tersebut:

```yaml
- address: 10.0.0.12
  port: 8080
  metrics_instance: "backend-b.internal"
```

Dua backend di host yang sama akan berbagi satu identitas metrik dan karenanya mendapat bobot yang
sama. Kalau itu bukan yang kamu mau, bedakan lewat `metrics_instance`.

## Apa yang terjadi kalau metriknya hilang

Ini bagian yang paling menentukan apakah fitur ini aman dipakai di produksi.

| Kejadian | Perilaku |
| --- | --- |
| Prometheus tidak bisa dihubungi | Bobot terakhir dipertahankan. `xdplb_weight_refresh_failed_total` naik. Trafik tidak terganggu. |
| Query sukses tapi satu backend tidak punya seri | Backend itu mempertahankan bobot sebelumnya. **Tidak** dianggap berkapasitas nol. |
| Nilai 0, negatif, atau NaN | Seri itu diabaikan, bobot sebelumnya dipertahankan. |
| Semua seri tidak terpakai | Tidak ada bobot yang diubah. |

Pilihan desainnya sengaja: metrik yang hilang berarti *tidak tahu*, bukan *tidak punya kapasitas*.
Kalau seri yang hilang diperlakukan sebagai nol, satu exporter mati akan mengeluarkan backend yang
sehat dari rotasi — mengubah masalah observability jadi outage.

Health check tetap jalan terpisah dan tetap berwenang mengeluarkan backend. Bobot hanya mengatur
proporsi di antara backend yang sudah dinyatakan sehat.

## Batas tabel maglev

Tabel maglev punya 4099 slot. Jumlah kandidat adalah total bobot seluruh backend dalam satu service,
jadi `max_weight` yang tinggi dikali banyak backend bisa mendekati atau melewati jumlah slot.

Config divalidasi saat startup: `jumlah_backend x max_weight` yang melewati 4099 ditolak, dan yang
melewati 4099/8 hanya diberi peringatan — masih jalan, tapi porsi trafik akan menyimpang dari bobot
yang diminta karena slot per kandidat terlalu sedikit.

Default `max_weight: 16` memberi 256 kandidat untuk 16 backend, atau rasio 16 slot per kandidat.

## Membuktikannya jalan

Rig test punya Prometheus tiruan yang nilainya dibaca dari sebuah file, jadi kamu bisa menggeser
kapasitas kapan saja tanpa menyentuh Prometheus sungguhan.

```
make netns-up
sudo ip netns exec lb python3 test/fake-prometheus.py 9091 &
echo '{"10.0.0.11": 100, "10.0.0.12": 100}' | sudo tee /tmp/xdp-lb-scores.json
sudo ip netns exec lb ./target/debug/xdp-lb --config test/config.weighted.yaml
```

Hasil yang terukur saat backend kedua diturunkan ke kapasitas 25%:

```
$ echo '{"10.0.0.11": 100, "10.0.0.12": 25}' | sudo tee /tmp/xdp-lb-scores.json
$ curl -s localhost:9500/metrics | grep backend_weight
xdplb_backend_weight{service="web",backend="10.0.0.11:8080"} 16
xdplb_backend_weight{service="web",backend="10.0.0.12:8080"} 4

$ REQUESTS=60 make smoke
requests: 60
  be1: 48
  be2: 12          <- tepat 4:1, sama dengan rasio bobotnya
  failed: 0
```

Lalu matikan Prometheus tiruannya:

```
xdplb_weight_refresh_failed_total 1
xdplb_backend_weight{...10.0.0.11:8080"} 16    <- bobot terakhir dipertahankan
xdplb_backend_weight{...10.0.0.12:8080"} 16
requests: 20 ... failed: 0                     <- trafik tidak terganggu
```

Catatan kejujuran soal angka: `make smoke` memakai sampel kecil dan port sumber yang berurutan, jadi
pembagian pada bobot yang sama bisa terlihat miring (pernah terukur 20/40 dari 60 request). Yang
konsisten dan bisa diandalkan adalah rasio saat bobotnya memang berbeda. Untuk mengukur distribusi
dengan benar dibutuhkan port sumber acak dan sampel jauh lebih besar.
