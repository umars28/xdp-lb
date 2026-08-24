# Benchmark

## Apa yang diukur di sini, dan apa yang tidak

Yang diukur: **biaya CPU per paket di dalam program XDP**, lewat `BPF_PROG_TEST_RUN`. Kernel
menjalankan program terhadap paket sintetis dan melaporkan waktu yang dihabiskan di dalamnya. Tidak
ada NIC, tidak ada driver, tidak ada generator trafik yang berebut CPU.

Yang **tidak** diukur: throughput sistem. Angka di sini tidak bisa dibandingkan dengan angka pps
nginx atau IPVS, karena tidak menyertakan driver, alokasi buffer, penjadwalan softirq, maupun
kontensi. Untuk perbandingan semacam itu dibutuhkan dua mesin bare-metal, dan itu masih ada di
roadmap.

Kolom `Mpkt/s/core` adalah satu detik dibagi biaya per paket. Itu **batas atas** yang dipaksakan
datapath pada satu core, bukan angka yang akan kamu lihat di produksi.

```
make bench      # butuh root
```

## Kenapa metodenya tidak sesederhana yang terlihat

`BPF_PROG_TEST_RUN` punya parameter `repeat` yang menjalankan program ribuan kali dalam satu syscall
lalu melaporkan rata-rata. Terlihat sempurna untuk microbenchmark. Percobaan pertama memakainya, dan
hasilnya mustahil:

```
dsr, established flow                    TX       10.0         100.00
nat, established flow                    TX       30.0          33.33
```

DSR terukur tiga kali lebih cepat daripada NAT, padahal DSR menambah header IPv4 penuh dan menghitung
checksum dari nol sementara NAT hanya mengedit beberapa field. Dan 10 ns itu sama persis dengan biaya
jalur non-IP yang keluar di baris pertama fungsi.

Penyebabnya: **kernel tidak memulihkan isi paket antar repeat.** Diverifikasi, bukan diduga:

```
1000 in-kernel repeats of one nat flow produced miss=1 hit=0 pass=999
```

Hanya repeat pertama yang menempuh jalur NAT. Repeat berikutnya melihat paket yang destination-nya
sudah ditulis ulang ke backend, yang 5-tuple-nya tidak cocok dengan entry conntrack mana pun dan
tidak cocok dengan service mana pun, lalu keluar lewat `XDP_PASS`. Untuk DSR lebih parah: repeat
kedua melihat header luar IPIP, yang bukan TCP maupun UDP, jadi keluar di pemeriksaan protokol.

Jadi 999 dari 1000 sampel mengukur jalur yang salah. Perilaku ini sekarang dijaga test
(`prog_test_run_does_not_restore_the_packet_between_repeats`) supaya tidak menjebak orang lain.

**Metode yang dipakai sekarang:** setiap jalur yang memodifikasi paket diukur satu panggilan
sekaligus, masing-masing diberi salinan paket yang bersih. Konsekuensinya kernel menghitung waktu
untuk satu invokasi alih-alih mengamortisasi jam selama sejuta — jadi ada biaya tambahan per
panggilan. Biaya itu dikalibrasi pada jalur yang tidak menyentuh paket, tempat kedua metode
sama-sama valid, lalu dikurangkan:

```
non-ip frame, 1000000 in-kernel repeats : 11 ns/packet
non-ip frame, 20000 single calls        : 37 ns/packet
per-call overhead subtracted below      : 26 ns
```

Setelah dikurangkan, jalur non-IP kembali ke 11 ns — kalibrasinya konsisten dengan dirinya sendiri.

## Hasil

### aarch64, kernel 6.8.0-137, 4 core (Lima VM di Apple Silicon)

| Skenario | Verdict | ns/paket | Mpkt/s/core |
| --- | --- | --- | --- |
| non-ip frame, diteruskan | PASS | 11 | 91,5 |
| destinasi tak dikenal, diteruskan | PASS | 37 | 27,3 |
| nat, flow berjalan | TX | 67 | 14,9 |
| dsr, flow berjalan | TX | 72 | 13,8 |
| tanpa backend, di-drop | DROP | 86 | 11,6 |
| rate limited, di-drop | DROP | 271 | 3,7 |
| nat, flow baru | TX | 475 | 2,1 |
| dsr, flow baru | TX | 328 | 3,1 |

## Yang bisa dibaca dari angka ini

**Flow berjalan jauh lebih murah daripada flow baru** — 67 ns lawan 475 ns untuk NAT, selisih tujuh
kali. Ini alasan conntrack ada. Dari pengukuran trafik nyata sebelumnya, 440 dari 480 paket adalah
conntrack hit, jadi jalur murah itu yang mendominasi. Kalau maglev dihitung ulang tiap paket, biaya
rata-rata akan naik hampir sepuluh kali.

**DSR lebih mahal per paket di jalur berjalan** (72 lawan 67 ns) tapi **lebih murah di jalur flow
baru** (328 lawan 475 ns). Yang kedua bukan kejutan kalau desainnya diingat: NAT menulis dua entry
conntrack, satu untuk tiap arah, sementara DSR hanya menulis satu karena balasan tidak akan pernah
lewat load balancer. Selisih 147 ns itu kira-kira harga satu insert ke LRU hash — dan itu sekaligus
validasi silang bahwa yang diukur benar-benar jalur yang dimaksud.

Perlu diingat: keunggulan DSR yang sebenarnya bukan di biaya per paket, tapi di **jumlah paket yang
harus diproses sama sekali**. Diukur di rig netns, DSR memotong 42% paket dan 47% byte dari beban LB
untuk beban kerja yang sama. Lihat `docs/FORWARDING.md`.

**Rate limiting mahal** (271 ns) dibanding drop biasa (86 ns). Selisihnya adalah lookup dan update ke
LRU per-CPU hash. Ini pertukaran yang wajar untuk mekanisme pertahanan yang hanya aktif di pembentukan
flow baru, bukan di tiap paket — tapi bukan sesuatu yang gratis, dan itulah sebabnya rate limiting
mati secara default.

**Jalur PASS murah** (11 ns untuk non-IP, 37 ns setelah parsing penuh). Ini penting karena trafik yang
tidak ditujukan ke VIP mana pun tetap harus melewati program ini. Biaya yang dibebankan XDP pada
trafik yang tidak berkepentingan mendekati nol.

## Batasan yang jujur

- Semua angka dari VM, bukan bare-metal. Biaya absolutnya akan berbeda di perangkat lain; yang lebih
  bisa dipegang adalah **rasio antar skenario**, bukan nilai mutlaknya.
- Metode kalibrasi mengasumsikan biaya tambahan per panggilan konstan antar skenario. Itu wajar
  karena biaya tersebut ada di kernel di luar program, tapi tetap sebuah asumsi.
- Belum ada perbandingan dengan nginx atau IPVS. Itu pekerjaan yang berbeda dan butuh perangkat yang
  berbeda.
