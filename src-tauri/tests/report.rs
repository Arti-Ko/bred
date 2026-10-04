//! Отчёт о проблеме доходит до Telegram в том виде, в каком задуман.
//!
//! Настоящий Telegram в тесте не нужен и вреден: подставляем свой крошечный
//! сервер на месте Bot API и смотрим, что именно к нему пришло. Отчёт — всегда
//! одно сообщение: карточка текстом или подписью к группе вложений.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use bred_lib::{domain::Id, report};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

/// Запрос, как его увидел поддельный Telegram: путь и тело.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

async fn fake_telegram() -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut reader = BufReader::new(read);
                loop {
                    let mut request = String::new();
                    if reader.read_line(&mut request).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
                    let mut length = 0usize;
                    let mut chunked = false;
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).await.unwrap();
                        let header = header.trim_end();
                        if header.is_empty() {
                            break;
                        }
                        let lower = header.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("content-length:") {
                            length = value.trim().parse().unwrap();
                        }
                        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
                            chunked = true;
                        }
                    }
                    let mut body = Vec::new();
                    if chunked {
                        loop {
                            let mut size = String::new();
                            reader.read_line(&mut size).await.unwrap();
                            let size = usize::from_str_radix(size.trim(), 16).unwrap();
                            let mut chunk = vec![0u8; size + 2];
                            reader.read_exact(&mut chunk).await.unwrap();
                            if size == 0 {
                                break;
                            }
                            body.extend_from_slice(&chunk[..size]);
                        }
                    } else {
                        body.resize(length, 0);
                        reader.read_exact(&mut body).await.unwrap();
                    }
                    let reply = if path.ends_with("/sendMessage") {
                        r#"{"ok":true,"result":{"message_id":77}}"#
                    } else {
                        r#"{"ok":true,"result":[]}"#
                    };
                    log.lock()
                        .unwrap()
                        .push((path, String::from_utf8_lossy(&body).to_string()));
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        reply.len(),
                        reply
                    );
                    write.write_all(response.as_bytes()).await.unwrap();
                }
            });
        }
    });
    (address, seen)
}

fn file(dir: &std::path::Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, [7u8; 256]).unwrap();
    path
}

#[tokio::test]
async fn report_arrives_as_card_with_replies() {
    let (api, seen) = fake_telegram().await;
    std::env::set_var("BRED_REPORT_API", &api);
    std::env::set_var("BRED_REPORT_TOKEN", "123:test");
    std::env::set_var("BRED_REPORT_CHAT", "-1001");

    let dir = std::env::temp_dir().join(format!("bred-report-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let draft = report::Draft {
        kind: "audio".into(),
        title: "Хрипит <звук>".into(),
        body: "Минут через десять разговора & дальше".into(),
        files: vec![
            file(&dir, "снимок.png"),
            file(&dir, "запись.mov"),
            file(&dir, "смешно.gif"),
        ],
        with_log: true,
    };
    let context = report::Context {
        nick: "марина".into(),
        account: Id([3u8; 32]),
        device: Id([4u8; 32]),
        network: "соседей 2".into(),
        log: Some("строка журнала".into()),
        sent_at: "03.10.2026 10:42".into(),
    };

    let reporter = report::Reporter::new();
    let id = reporter
        .send(draft.clone(), context)
        .await
        .expect("отчёт ушёл");
    assert!(id.starts_with("R-"));

    let seen = seen.lock().unwrap().clone();
    let paths: Vec<&str> = seen.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        vec!["/bot123:test/sendMediaGroup"],
        "одно сообщение: фото, видео, GIF и журнал одной группой файлов"
    );

    let group = &seen[0].1;
    assert!(
        group.contains("Хрипит &lt;звук&gt;"),
        "текст человека экранирован"
    );
    assert!(group.contains("&amp; дальше"));
    assert!(
        group.contains(&format!("#{}", id.replace('-', "_"))),
        "номер отчёта — хэштегом"
    );
    assert!(group.contains("#баг #звук"));
    for index in 0..4 {
        assert!(
            group.contains(&format!("attach://file{index}")),
            "вложение {index}"
        );
    }
    assert!(group.contains("строка журнала"), "журнал — в той же группе");
    assert!(
        !group.contains("reply_parameters"),
        "отвечать не на что: сообщение одно"
    );

    // Второй отчёт сразу следом — отказ с объяснением, а не тишина.
    let again = reporter
        .send(
            draft,
            report::Context {
                nick: "марина".into(),
                account: Id([3u8; 32]),
                device: Id([4u8; 32]),
                network: String::new(),
                log: None,
                sent_at: String::new(),
            },
        )
        .await;
    assert!(again.unwrap_err().to_string().contains("подождите"));

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn photo_alone_comes_as_one_captioned_photo() {
    let (api, seen) = fake_telegram().await;
    let dir = std::env::temp_dir().join(format!("bred-report-photo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bot = report::telegram::Bot::new(&api, "123:test", "-1001").unwrap();
    bot.send_bundle(
        vec![report::telegram::Media {
            kind: report::telegram::MediaKind::Photo,
            name: "снимок.png".into(),
            mime: "image/png".into(),
            bytes: vec![7u8; 64],
        }],
        "карточка",
    )
    .await
    .unwrap();
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "одно сообщение");
    assert!(seen[0].0.ends_with("/sendPhoto"));
    assert!(seen[0].1.contains("карточка"), "карточка — подписью к фото");
    std::fs::remove_dir_all(&dir).ok();
}

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAGAAAABACAIAAABqVuVZAAAwaElEQVR42hTa8Uc7cRzH8cnk5OTk5ORkMpmcnExOzkxOJicnk8lkMjmZTCaTyVMmk8lkMplMJpPJZDKZ7If9sB/2w37YD/thP+yH/pBv3/sTHu7zeb/fr/fH4/Ew58HrYd6D4GHBg+hh0YPkYcmD7GHZg+JhxYPqYdWDz8OaB7+HdQ8BDxseNA+bHnQPWx6CHrY9GB52PJgeQh7CHnY9WB72PEQ87HuwPRx4cDwceoh6OPIQ83DsIe7hxEPCw6mHpIczD66Hcw8pDxce0h4uPWQ8XHnIerj2kPNw4wEPtx7yHu48FDzceyh6ePBQ8vDooezhyUPFw7OHqocXDzUPrx7qHt48NDy8e2h6+PDQ8vDpoe3hy0PHw7eHrocfDx6PlzkvXi/zXgQvC15EL4teJC9LXmQvy14ULyteVC+rXnxe1rz4vax7CXjZ8KJ52fSie9nyEvSy7cXwsuPF9BLyEvay68Xysucl4mXfi+3lwIvj5dBL1MuRl5iXYy9xLydeEl5OvSS9nHlxvZx7SXm58JL2cukl4+XKS9bLtZeclxsveLn1kvdy56Xg5d5L0cuDl5KXRy9lL09eKl6evVS9vHipeXn1Uvfy5qXh5d1L08uHl5aXTy9tL19eOl6+vXS9/Hj/gATmBLwC8wKCwIKAKLAoIAksCcgCywKKwIqAKrAq4BNYE/ALrAsEBDYENIFNAV1gSyAosC1gCOwImAIhgbDAroAlsCcQEdgXsAUOBByBQ4GowJFATOBYIC5wIpAQOBVICpwJuALnAimBC4G0wKVARuBKICtwLZATuBFA4FYgL3AnUBC4FygKPAiUBB4FygJPAhWBZ4GqwItATeBVoC7wJtAQeBdoCnwItAQ+BdoCXwIdgW+BrsCP8AckMifiFZkXEUQWRESRRRFJZElEFlkWUURWRFSRVRGfyJqIX2RdJCCyIaKJbIroIlsiQZFtEUNkR8QUCYmERXZFLJE9kYjIvogtciDiiByKREWORGIixyJxkRORhMipSFLkTMQVORdJiVyIpEUuRTIiVyJZkWuRnMiNCCK3InmRO5GCyL1IUeRBpCTyKFIWeRKpiDyLVEVeRGoiryJ1kTeRhsi7SFPkQ6Ql8inSFvkS6Yh8i3RFfsQ/IIk5Ca/EvIQgsSAhSixKSBJLErLEsoQisSKhSqxK+CTWJPwS6xIBiQ0JTWJTQpfYkghKbEsYEjsSpkRIIiyxK2FJ7ElEJPYlbIkDCUfiUCIqcSQRkziWiEucSCQkTiWSEmcSrsS5REriQiItcSmRkbiSyEpcS+QkbiSQuJXIS9xJFCTuJYoSDxIliUeJssSTREXiWaIq8SJRk3iVqEu8STQk3iWaEh8SLYlPibbEl0RH4luiK/Ej/QHJzMl4ZeZlBJkFGVFmUUaSWZKRZZZlFJkVGVVmVcYnsybjl1mXCchsyGgymzK6zJZMUGZbxpDZkTFlQjJhmV0ZS2ZPJiKzL2PLHMg4MocyUZkjmZjMsUxc5kQmIXMqk5Q5k3FlzmVSMhcyaZlLmYzMlUxW5lomJ3Mjg8ytTF7mTqYgcy9TlHmQKck8ypRlnmQqMs8yVZkXmZrMq0xd5k2mIfMu05T5kGnJfMq0Zb5kOjLfMl2ZH/kPSGFOwaswryAoLCiICosKksKSgqywrKAorCioCqsKPoU1Bb/CukJAYUNBU9hU0BW2FIIK2wqGwo6CqRBSCCvsKlgKewoRhX0FW+FAwVE4VIgqHCnEFI4V4gonCgmFU4WkwpmCq3CukFK4UEgrXCpkFK4UsgrXCjmFGwUUbhXyCncKBYV7haLCg0JJ4VGhrPCkUFF4VqgqvCjUFF4V6gpvCg2Fd4WmwodCS+FToa3wpdBR+FboKvwof0AqcypelXkVQWVBRVRZVJFUllRklWUVRWVFRVVZVfGprKn4VdZVAiobKprKpoqusqUSVNlWMVR2VEyVkEpYZVfFUtlTiajsq9gqByqOyqFKVOVIJaZyrBJXOVFJqJyqJFXOVFyVc5WUyoVKWuVSJaNypZJVuVbJqdyooHKrkle5Uymo3KsUVR5USiqPKmWVJ5WKyrNKVeVFpabyqlJXeVNpqLyrNFU+VFoqnyptlS+Vjsq3SlflR/0D8jHnw+tj3ofgY8GH6GPRh+RjyYfsY9mH4mPFh+pj1YfPx5oPv491HwEfGz40H5s+dB9bPoI+tn0YPnZ8mD5CPsI+dn1YPvZ8RHzs+7B9HPhwfBz6iPo48hHzcewj7uPER8LHqY+kjzMfro9zHykfFz7SPi59ZHxc+cj6uPaR83HjAx+3PvI+7nwUfNz7KPp48FHy8eij7OPJR8XHs4+qjxcfNR+vPuo+3nw0fLz7aPr48NHy8emj7ePLR8fHt4+ujx/fH5CfOT9eP/N+BD8LfkQ/i34kP0t+ZD/LfhQ/K35UP6t+fH7W/Pj9rPsJ+Nnwo/nZ9KP72fIT9LPtx/Cz48f0E/IT9rPrx/Kz5yfiZ9+P7efAj+Pn0E/Uz5GfmJ9jP3E/J34Sfk79JP2c+XH9nPtJ+bnwk/Zz6Sfj58pP1s+1n5yfGz/4ufWT93Pnp+Dn3k/Rz4Ofkp9HP2U/T34qfp79VP28+Kn5efVT9/Pmp+Hn3U/Tz4eflp9PP20/X346fr79dP38+P+AAswF8AaYDyAEWAggBlgMIAVYCiAHWA6gBFgJoAZYDeALsBbAH2A9QCDARgAtwGYAPcBWgGCA7QBGgJ0AZoBQgHCA3QBWgL0AkQD7AewABwGcAIcBogGOAsQCHAeIBzgJkAhwGiAZ4CyAG+A8QCrARYB0gMsAmQBXAbIBrgPkAtwEIMBtgHyAuwCFAPcBigEeApQCPAYoB3gKUAnwHKAa4CVALcBrgHqAtwCNAO8BmgE+ArQCfAZoB/gK0AnwHaAb4CfwB6Qxp+HVmNcQNBY0RI1FDUljSUPWWNZQNFY0VI1VDZ/GmoZfY10joLGhoWlsaugaWxpBjW0NQ2NHw9QIaYQ1djUsjT2NiMa+hq1xoOFoHGpENY40YhrHGnGNE42ExqlGUuNMw9U410hpXGikNS41MhpXGlmNa42cxo0GGrcaeY07jYLGvUZR40GjpPGoUdZ40qhoPGtUNV40ahqvGnWNN42GxrtGU+NDo6XxqdHW+NLoaHxrdDV+tD8gnTkdr868jqCzoCPqLOpIOks6ss6yjqKzoqPqrOr4dNZ0/DrrOgGdDR1NZ1NH19nSCeps6xg6OzqmTkgnrLOrY+ns6UR09nVsnQMdR+dQJ6pzpBPTOdaJ65zoJHROdZI6ZzquzrlOSudCJ61zqZPRudLJ6lzr5HRudNC51cnr3OkUdO51ijoPOiWdR52yzpNORedZp6rzolPTedWp67zpNHTedZo6HzotnU+dts6XTkfnW6er86P/AQWZC+INMh9ECLIQRAyyGEQKshREDrIcRAmyEkQNshrEF2QtiD/IepBAkI0gWpDNIHqQrSDBINtBjCA7QcwgoSDhILtBrCB7QSJB9oPYQQ6COEEOg0SDHAWJBTkOEg9yEiQR5DRIMshZEDfIeZBUkIsg6SCXQTJBroJkg1wHyQW5CUKQ2yD5IHdBCkHugxSDPAQpBXkMUg7yFKQS5DlINchLkFqQ1yD1IG9BGkHegzSDfARpBfkM0g7yFaQT5DtIN8hP8A/IYM7AazBvIBgsGIgGiwaSwZKBbLBsoBisGKgGqwY+gzUDv8G6QcBgw0Az2DTQDbYMggbbBobBjoFpEDIIG+waWAZ7BhGDfQPb4MDAMTg0iBocGcQMjg3iBicGCYNTg6TBmYFrcG6QMrgwSBtcGmQMrgyyBtcGOYMbAwxuDfIGdwYFg3uDosGDQcng0aBs8GRQMXg2qBq8GNQMXg3qBm8GDYN3g6bBh0HL4NOgbfBl0DH4Nuga/Bh/QCZzJl6TeRPBZMFENFk0kUyWTGSTZRPFZMVENVk18ZmsmfhN1k0CJhsmmsmmiW6yZRI02TYxTHZMTJOQSdhk18Qy2TOJmOyb2CYHJo7JoUnU5MgkZnJsEjc5MUmYnJokTc5MXJNzk5TJhUna5NIkY3JlkjW5NsmZ3JhgcmuSN7kzKZjcmxRNHkxKJo8mZZMnk4rJs0nV5MWkZvJqUjd5M2mYvJs0TT5MWiafJm2TL5OOybdJ1+TH/AMKMxfGG2Y+jBBmIYwYZjGMFGYpjBxmOYwSZiWMGmY1jC/MWhh/mPUwgTAbYbQwm2H0MFthgmG2wxhhdsKYYUJhwmF2w1hh9sJEwuyHscMchHHCHIaJhjkKEwtzHCYe5iRMIsxpmGSYszBumPMwqTAXYdJhLsNkwlyFyYa5DpMLcxOGMLdh8mHuwhTC3IcphnkIUwrzGKYc5ilMJcxzmGqYlzC1MK9h6mHewjTCvIdphvkI0wrzGaYd5itMJ8x3mG6Yn/AfkMWchddi3kKwWLAQLRYtJIslC9li2UKxWLFQLVYtfBZrFn6LdYuAxYaFZrFpoVtsWQQtti0Mix0L0yJkEbbYtbAs9iwiFvsWtsWBhWNxaBG1OLKIWRxbxC1OLBIWpxZJizML1+LcImVxYZG2uLTIWFxZZC2uLXIWNxZY3FrkLe4sChb3FkWLB4uSxaNF2eLJomLxbFG1eLGoWbxa1C3eLBoW7xZNiw+LlsWnRdviy6Jj8W3Rtfix/oAizEXwRpiPIERYiCBGWIwgRViKIEdYjqBEWImgRliN4IuwFsEfYT1CIMJGBC3CZgQ9wlaEYITtCEaEnQhmhFCEcITdCFaEvQiRCPsR7AgHEZwIhxGiEY4ixCIcR4hHOImQiHAaIRnhLIIb4TxCKsJFhHSEywiZCFcRshGuI+Qi3EQgwm2EfIS7CIUI9xGKER4ilCI8RihHeIpQifAcoRrhJUItwmuEeoS3CI0I7xGaET4itCJ8RmhH+IrQifAdoRvhJ/IHZDNn47WZtxFsFmxEm0UbyWbJRrZZtlFsVmxUm1Ubn82ajd9m3SZgs2Gj2Wza6DZbNkGbbRvDZsfGtAnZhG12bSybPZuIzb6NbXNg49gc2kRtjmxiNsc2cZsTm4TNqU3S5szGtTm3Sdlc2KRtLm0yNlc2WZtrm5zNjQ02tzZ5mzubgs29TdHmwaZk82hTtnmyqdg821RtXmxqNq82dZs3m4bNu03T5sOmZfNp07b5sunYfNt0bX7sPyCHOQevw7yD4LDgIDosOkgOSw6yw7KD4rDioDqsOvgc1hz8DusOAYcNB81h00F32HIIOmw7GA47DqZDyCHssOtgOew5RBz2HWyHAwfH4dAh6nDkEHM4dog7nDgkHE4dkg5nDq7DuUPK4cIh7XDpkHG4csg6XDvkHG4ccLh1yDvcORQc7h2KDg8OJYdHh7LDk0PF4dmh6vDiUHN4dag7vDk0HN4dmg4fDi2HT4e2w5dDx+Hboevw4/wBRZmL4o0yH0WIshBFjLIYRYqyFEWOshxFibISRY2yGsUXZS2KP8p6lECUjShalM0oepStKMEo21GMKDtRzCihKOEou1GsKHtRIlH2o9hRDqI4UQ6jRKMcRYlFOY4Sj3ISJRHlNEoyylkUN8p5lFSUiyjpKJdRMlGuomSjXEfJRbmJQpTbKPkod1EKUe6jFKM8RClFeYxSjvIUpRLlOUo1ykuUWpTXKPUob1EaUd6jNKN8RGlF+YzSjvIVpRPlO0o3yk/0DyjGXAxvjPkYQoyFGGKMxRhSjKUYcozlGEqMlRhqjNUYvhhrMfwx1mMEYmzE0GJsxtBjbMUIxtiOYcTYiWHGCMUIx9iNYcXYixGJsR/DjnEQw4lxGCMa4yhGLMZxjHiMkxiJGKcxkjHOYrgxzmOkYlzESMe4jJGJcRUjG+M6Ri7GTQxi3MbIx7iLUYhxH6MY4yFGKcZjjHKMpxiVGM8xqjFeYtRivMaox3iL0YjxHqMZ4yNGK8ZnjHaMrxidGN8xujF+Yn9AcebieOPMxxHiLMQR4yzGkeIsxZHjLMdR4qzEUeOsxvHFWYvjj7MeJxBnI44WZzOOHmcrTjDOdhwjzk4cM04oTjjObhwrzl6cSJz9OHacgzhOnMM40ThHcWJxjuPE45zEScQ5jZOMcxbHjXMeJxXnIk46zmWcTJyrONk413FycW7iEOc2Tj7OXZxCnPs4xTgPcUpxHuOU4zzFqcR5jlON8xKnFuc1Tj3OW5xGnPc4zTgfcVpxPuO043zF6cT5jtON8xP/A0owl8CbYD6BkGAhgZhgMYGUYCmBnGA5gZJgJYGaYDWBL8FaAn+C9QSBBBsJtASbCfQEWwmCCbYTGAl2EpgJQgnCCXYTWAn2EkQS7CewExwkcBIcJogmOEoQS3CcIJ7gJEEiwWmCZIKzBG6C8wSpBBcJ0gkuE2QSXCXIJrhOkEtwk4AEtwnyCe4SFBLcJygmeEhQSvCYoJzgKUElwXOCaoKXBLUErwnqCd4SNBK8J2gm+EjQSvCZoJ3gK0EnwXeCboKfxB9Qkrkk3iTzSYQkC0nEJItJpCRLSeQky0mUJCtJ1CSrSXxJ1pL4k6wnCSTZSKIl2UyiJ9lKEkyyncRIspPETBJKEk6ym8RKspckkmQ/iZ3kIImT5DBJNMlRkliS4yTxJCdJEklOkySTnCVxk5wnSSW5SJJOcpkkk+QqSTbJdZJckpskJLlNkk9yl6SQ5D5JMclDklKSxyTlJE9JKkmek1STvCSpJXlNUk/ylqSR5D1JM8lHklaSzyTtJF9JOkm+k3ST/CT/gFzmXLwu8y6Cy4KL6LLoIrksucguyy6Ky4qL6rLq4nNZc/G7rLsEXDZcNJdNF91lyyXosu1iuOy4mC4hl7DLrovlsucScdl3sV0OXByXQ5eoy5FLzOXYJe5y4pJwOXVJupy5uC7nLimXC5e0y6VLxuXKJety7ZJzuXHB5dYl73LnUnC5dym6PLiUXB5dyi5PLhWXZ5eqy4tLzeXVpe7y5tJweXdpuny4tFw+XdouXy4dl2+XrsuP+weUYi6FN8V8CiHFQgoxxWIKKcVSCjnFcgolxUoKNcVqCl+KtRT+FOspAik2UmgpNlPoKbZSBFNspzBS7KQwU4RShFPsprBS7KWIpNhPYac4SOGkOEwRTXGUIpbiOEU8xUmKRIrTFMkUZyncFOcpUikuUqRTXKbIpLhKkU1xnSKX4iYFKW5T5FPcpSikuE9RTPGQopTiMUU5xVOKSornFNUULylqKV5T1FO8pWikeE/RTPGRopXiM0U7xVeKTorvFN0UP6k/oDRzabxp5tMIaRbSiGkW00hpltLIaZbTKGlW0qhpVtP40qyl8adZTxNIs5FGS7OZRk+zlSaYZjuNkWYnjZkmlCacZjeNlWYvTSTNfho7zUEaJ81hmmiaozSxNMdp4mlO0iTSnKZJpjlL46Y5T5NKc5EmneYyTSbNVZpsmus0uTQ3aUhzmyaf5i5NIc19mmKahzSlNI9pymme0lTSPKeppnlJU0vzmqae5i1NI817mmaajzStNJ9p2mm+0nTSfKfppvlJ/wFlmMvgzTCfQciwkEHMsJhByrCUQc6wnEHJsJJBzbCawZdhLYM/w3qGQIaNDFqGzQx6hq0MwQzbGYwMOxnMDKEM4Qy7GawMexkiGfYz2BkOMjgZDjNEMxxliGU4zhDPcJIhkeE0QzLDWQY3w3mGVIaLDOkMlxkyGa4yZDNcZ8hluMlAhtsM+Qx3GQoZ7jMUMzxkKGV4zFDO8JShkuE5QzXDS4ZahtcM9QxvGRoZ3jM0M3xkaGX4zNDO8JWhk+E7QzfDT+YPKMtcFm+W+SxCloUsYpbFLFKWpSxyluUsSpaVLGqW1Sy+LGtZ/FnWswSybGTRsmxm0bNsZQlm2c5iZNnJYmYJZQln2c1iZdnLEsmyn8XOcpDFyXKYJZrlKEssy3GWeJaTLIksp1mSWc6yuFnOs6SyXGRJZ7nMkslylSWb5TpLLstNFrLcZslnuctSyHKfpZjlIUspy2OWcpanLJUsz1mqWV6y1LK8ZqlnecvSyPKepZnlI0sry2eWdpavLJ0s31m6WX6yf0A55nJ4c8znEHIs5BBzLOaQcizlkHMs51ByrORQc6zm8OVYy+HPsZ4jkGMjh5ZjM4eeYytHMMd2DiPHTg4zRyhHOMduDivHXo5Ijv0cdo6DHE6OwxzRHEc5YjmOc8RznORI5DjNkcxxlsPNcZ4jleMiRzrHZY5Mjqsc2RzXOXI5bnKQ4zZHPsddjkKO+xzFHA85Sjkec5RzPOWo5HjOUc3xkqOW4zVHPcdbjkaO9xzNHB85Wjk+c7RzfOXo5PjO0c3xk/sDgrn/b0iY//9QgoX/rwFY/L/yZun/Xpfl/8tLVv5v6Fj9v4Zi7f+uhfX/CwU2/qfmbP6Phtn6n3+y/T/kY+d/kkXof1zD7v9Mgr3/gzf7/6dLDv6PUBz+nxM4+t8Mc/y/4+Pkf1vD6f/azdn/AsX5/1uYi/9XDZf/zxNX/38arv/LcMP/7xbycAcFuIciPEAJHqEMT1CBZ6jCC9TgFerwBg14hyZ8QAs+oQ1f0IFv6MIPf0B55vJ488znEfIs5BHzLOaR8izlkfMs51HyrORR86zm8eVZy+PPs54nkGcjj5ZnM4+eZytPMM92HiPPTh4zTyhPOM9uHivPXp5Inv08dp6DPE6ewzzRPEd5YnmO88TznORJ5DnNk8xzlsfNc54nleciTzrPZZ5Mnqs82TzXeXJ5bvL/bW7z5PPc5Snkuc9TzPOQp5TnMU85z1OeSp7nPNU8L3lqeV7z1PO85Wnkec/TzPORp5XnM087z1eeTp7vPN08P/k/oAJzBbwF5gsIBRYKiAUWC0gFlgrIBZYLKAVWCqgFVgv4CqwV8BdYLxAosFFAK7BZQC+wVSBYYLuAUWCngFkgVCBcYLeAVWCvQKTAfgG7wEEBp8BhgWiBowKxAscF4gVOCiQKnBZIFjgr4BY4L5AqcFEgXeCyQKbAVYFsgesCuQI3hf+/zW2BfIG7AoUC9wWKBR4KlAo8FigXeCpQKfBcoFrgpUCtwGuBeoG3Ao0C7wWaBT4KtAp8FmgX+CrQKfBdoFvgp/AHVGSuiLfIfBGhyEIRschiEanIUhG5yHIRpchKEbXIahFfkbUi/iLrRQJFNopoRTaL6EW2igSLbBcxiuwUMYuEioSL7BaxiuwViRTZL2IXOSjiFDksEi1yVCRW5LhIvMhJkUSR0yLJImdF3CLnRVJFLoqki1wWyRS5KpItcl0kV+Sm+P9E3RbJF7krUihyX6RY5KFIqchjkXKRpyKVIs9FqkVeitSKvBapF3kr0ijyXqRZ5KNIq8hnkXaRryKdIt9FukV+in9AJeZKeEvMlxBKLJQQSyyWkEoslZBLLJdQSqyUUEuslvCVWCvhL7FeIlBio4RWYrOEXmKrRLDEdgmjxE4Js0SoRLjEbgmrxF6JSIn9EnaJgxJOicMS0RJHJWIljkvES5yUSJQ4LZEscVbCLXFeIlXiokS6xGWJTImrEtkS1yVyJW5K/y+b2xL5EnclCiXuSxRLPJQolXgsUS7xVKJS4rlEtcRLiVqJ1xL1Em8lGiXeSzRLfJRolfgs0S7xVaJT4rtEt8RP6Q+ozFwZb5n5MkKZhTJimcUyUpmlMnKZ5TJKmZUyapnVMr4ya2X8ZdbLBMpslNHKbJbRy2yVCZbZLmOU2SljlgmVCZfZLWOV2SsTKbNfxi5zUMYpc1gmWuaoTKzMcZl4mZMyiTKnZZJlzsq4Zc7LpMpclEmXuSyTKXNVJlvmukyuzE35/z18WyZf5q5Mocx9mWKZhzKlMo9lymWeylTKPJeplnkpUyvzWqZe5q1Mo8x7mWaZjzKtMp9l2mW+ynTKfJfplvkp/wFVmKvgrTBfQaiwUEGssFhBqrBUQa6wXEGpsFJBrbBawVdhrYK/wnqFQIWNClqFzQp6ha0KwQrbFYwKOxXMCqEK4Qq7FawKexUiFfYr2BUOKjgVDitEKxxViFU4rhCvcFIhUeG0QrLCWQW3wnmFVIWLCukKlxUyFa4qZCtcV8hVuKn8L1G3FfIV7ioUKtxXKFZ4qFCq8FihXOGpQqXCc4VqhZcKtQqvFeoV3io0KrxXaFb4qNCq8FmhXeGrQqfCd4VuhZ/KH1CVuSreKvNVhCoLVcQqi1WkKktV5CrLVZQqK1XUKqtVfFXWqvirrFcJVNmoolXZrKJX2aoSrLJdxaiyU8WsEqoSrrJbxaqyVyVSZb+KXeWgilPlsEq0ylGVWJXjKvEqJ1USVU6rJKucVXGrnFdJVbmokq5yWSVT5apKtsp1lVyVm+r/6n1bJV/lrkqhyn2VYpWHKqUqj1XKVZ6qVKo8V6lWealSq/JapV7lrUqjynuVZpWPKq0qn1XaVb6qdKp8V+lW+an+AdWYq+GtMV9DqLFQQ6yxWEOqsVRDrrFcQ6mxUkOtsVrDV2Othr/Geo1AjY0aWo3NGnqNrRrBGts1jBo7NcwaoRrhGrs1rBp7NSI19mvYNQ5qODUOa0RrHNWI1TiuEa9xUiNR47RGssZZDbfGeY1UjYsa6RqXNTI1rmpka1zXyNW4qf1vbG5r5Gvc1SjUuK9RrPFQo1TjsUa5xlONSo3nGtUaLzVqNV5r1Gu81WjUeK/RrPFRo1Xjs0a7xleNTo3vGt0aP7U/oDpzdbx15usIdRbqiHUW60h1lurIdZbrKHVW6qh1Vuv46qzV8ddZrxOos1FHq7NZR6+zVSdYZ7uOUWenjlknVCdcZ7eOVWevTqTOfh27zkEdp85hnWidozqxOsd14nVO6iTqnNZJ1jmr49Y5r5Oqc1EnXeeyTqbOVZ1snes6uTo39f89322dfJ27OoU693WKdR7qlOo81inXeapTqfNcp1rnpU6tzmudep23Oo0673WadT7qtOp81mnX+arTqfNdp1vnp/4H1GCugbfBfAOhwUIDscFiA6nBUgO5wXIDpcFKA7XBagNfg7UG/gbrDQINNhpoDTYb6A22GgQbbDcwGuw0MBuEGoQb7DawGuw1iDTYb2A3OGjgNDhsEG1w1CDW4LhBvMFJg0SD0wbJBmcN3AbnDVINLhqkG1w2yDS4apBtcN0g1+Cm8b8dvm2Qb3DXoNDgvkGxwUODUoPHBuUGTw0qDZ4bVBu8NKg1eG1Qb/DWoNHgvUGzwUeDVoPPBu0GXw06Db4bdBv8NP6Amsw18TaZbyI0WWgiNllsIjVZaiI3WW6iNFlpojZZbeJrstbE32S9SaDJRhOtyWYTvclWk2CT7SZGk50mZpNQk3CT3SZWk70mkSb7TewmB02cJodNok2OmsSaHDeJNzlpkmhy2iTZ5KyJ2+S8SarJRZN0k8smmSZXTbJNrpvkmtw0/08Kt03yTe6aFJrcNyk2eWhSavLYpNzkqUmlyXOTapOXJrUmr03qTd6aNJq8N2k2+WjSavLZpN3kq0mnyXeTbpOf5h9Qi7kW3hbzLYQWCy3EFostpBZLLeQWyy2UFist1BarLXwt1lr4W6y3CLTYaKG12Gyht9hqEWyx3cJosdPCbBFqEW6x28Jqsdci0mK/hd3ioIXT4rBFtMVRi1iL4xbxFictEi1OWyRbnLVwW5y3SLW4aJFucdki0+KqRbbFdYtci5vW/yHqtkW+xV2LQov7FsUWDy1KLR5blFs8tai0eG5RbfHSotbitUW9xVuLRov3Fs0WHy1aLT5btFt8tei0+G7RbfHT+gNqM9fG22a+jdBmoY3YZrGN1GapjdxmuY3SZqWN2ma1ja/NWht/m/U2gTYbbbQ2m230Nlttgm222xhtdtqYbUJtwm1221ht9tpE2uy3sdsctHHaHLaJtjlqE2tz3Cbe5qRNos1pm2SbszZum/M2qTYXbdJtLttk2ly1yba5bpNrc9P+P1/etsm3uWtTaHPfptjmoU2pzWObcpunNpU2z22qbV7a1Nq8tqm3eWvTaPPeptnmo02rzWebdpuvNp023226bX7af0Ad5jp4O8x3EDosdBA7LHaQOix1kDssd1A6rHRQO6x28HVY6+DvsN4h0GGjg9Zhs4PeYatDsMN2B6PDTgezQ6hDuMNuB6vDXodIh/0OdoeDDk6Hww7RDkcdYh2OO8Q7nHRIdDjtkOxw1sHtcN4h1eGiQ7rDZYdMh6sO2Q7XHXIdbjr/R+/bDvkOdx0KHe47FDs8dCh1eOxQ7vDUodLhuUO1w0uHWofXDvUObx0aHd47NDt8dGh1+OzQ7vDVodPhu0O3w0/nD6jLXBdvl/kuQpeFLmKXxS5Sl6UucpflLkqXlS5ql9Uuvi5rXfxd1rsEumx00bpsdtG7bHUJdtnuYnTZ6WJ2CXUJd9ntYnXZ6xLpst/F7nLQxely2CXa5ahLrMtxl3iXky6JLqddkl3Ourhdzrukulx0SXe57JLpctUl2+W6S67LTfd/KnHbJd/lrkuhy32XYpeHLqUuj13KXZ66VLo8d6l2eelS6/Lapd7lrUujy3uXZpePLq0un13aXb66dLp8d+l2+en+AfWY6+HtMd9D6LHQQ+yx2EPqsdRD7rHcQ+mx0kPtsdrD12Oth7/Heo9Aj40eWo/NHnqPrR7BHts9jB47PcweoR7hHrs9rB57PSI99nvYPQ56OD0Oe0R7HPWI9TjuEe9x0iPR47RHssdZD7fHeY9Uj4se6R6XPTI9rnpke1z3yPW46UGP2x75Hnc9Cj3uexR7PPQo9XjsUe7x1KPS47lHtcdLj1qP1x71Hm89Gj3eezR7fPRo9fjs0e7x1aPT47tHt8dP7w+oz1wfb5/5PkKfhT5in8U+Up+lPnKf5T5Kn5U+ap/VPr4+a338fdb7BPps9NH6bPbR+2z1CfbZ7mP02elj9gn1CffZ7WP12esT6bPfx+5z0Mfpc9gn2ueoT6zPcZ94n5M+iT6nfZJ9zvq4fc77pPpc9En3ueyT6XPVJ9vnuk+uz00f+tz2yfe561Poc9+n2OehT6nPY59yn6c+lT7Pfap9XvrU+rz2qfd569Po896n2eejT6vPZ592n68+nT7ffbp9fvp/QAPmBngHzA8QBiwMEAcsDpAGLA2QBywPUAasDFAHrA7wDVgb4B+wPiAwYGOANmBzgD5ga0BwwPYAY8DOAHNAaEB4wO4Aa8DegMiA/QH2gIMBzoDDAdEBRwNiA44HxAecDEgMOB2QHHA2wB1wPiA14GJAesDlgMyAqwHZAdcDcgNuBjDgdkB+wN2AwoD7AcUBDwNKAx4HlAc8DagMeB5QHfAyoDbgdUB9wNuAxoD3Ac0BHwNaAz4HtAd8DegM+B7QHfAz+AMaMjfEO2R+iDBkYYg4ZHGINGRpiDxkeYgyZGWIOmR1iG/I2hD/kPUhgSEbQ7Qhm0P0IVtDgkO2hxhDdoaYQ0JDwkN2h1hD9oZEhuwPsYccDHGGHA6JDjkaEhtyPCQ+5GRIYsjpkOSQsyHukPMhqSEXQ9JDLodkhlwNyQ65HpIbcjOEIbdD8kPuhhSG3A8pDnkYUhryOKQ85GlIZcjzkOqQlyG1Ia9D6kPehjSGvA9pDvkY0hryOaQ95GtIZ8j3kO6Qn+Ef0Ii5Ed4R8yOEEQsjxBGLI6QRSyPkEcsjlBErI9QRqyN8I9ZG+EesjwiM2BihjdgcoY/YGhEcsT3CGLEzwhwRGhEesTvCGrE3IjJif4Q94mCEM+JwRHTE0YjYiOMR8REnIxIjTkckR5yNcEecj0iNuBiRHnE5IjPiakR2xPWI3IibEYy4HZEfcTeiMOJ+RHHEw4jSiMcR5RFPIyojnkdUR7yMqI14HVEf8TaiMeJ9RHPEx4jWiM8R7RFfIzojvkd0R/yM/oDGzI3xjpkfI4xZGCOOWRwjjVkaI49ZHqOMWRmjjlkd4xuzNsY/Zn1MYMzGGG3M5hh9zNaY4JjtMcaYnTHmmNCY8JjdMdaYvTGRMftj7DEHY5wxh2OiY47GxMYcj4mPORmTGHM6JjnmbIw75nxMaszFmPSYyzGZMVdjsmOux+TG3IxhzO2Y/Ji7MYUx92OKYx7GlMY8jimPeRpTGfM8pjrmZUxtzOuY+pi3MY0x72OaYz7GtMZ8jmmP+RrTGfM9pjvmZ/wHNGFugnfC/ARhwsIEccLiBGnC0gR5wvIEZcLKBHXC6gTfhLUJ/gnrEwITNiZoEzYn6BO2JgQnbE8wJuxMMCeEJoQn7E6wJuxNiEzYn2BPOJjgTDicEJ1wNCE24XhCfMLJhMSE0wnJCWcT3AnnE1ITLiakJ1xOyEy4mpCdcD0hN+FmAhNuJ+Qn3E0oTLifUJzwMKE04XFCecLThMqE5wnVCS8TahNeJ9QnvE1oTHif0JzwMaE14XNCe8LXhM6E7wndCT+TP6Apc1O8U+anCFMWpohTFqdIU5amyFOWpyhTVqaoU1an+KasTfFPWZ8SmLIxRZuyOUWfsjUlOGV7ijFlZ4o5JTQlPGV3ijVlb0pkyv4Ue8rBFGfK4ZTolKMpsSnHU+JTTqYkppxOSU45m+JOOZ+SmnIxJT3lckpmytWU7JTrKbkpN1OYcjslP+VuSmHK/ZTilIcppSmPU8pTnqZUpjxPqU55mVKb8jqlPuVtSmPK+5TmlI8prSmfU9pTvqZ0pnxP6U75mf4BzZib4Z0xP0OYsTBDnLE4Q5qxNEOesTxDmbEyQ52xOsM3Y22Gf8b6jMCMjRnajM0Z+oytGcEZ2zOMGTszzBmhGeEZuzOsGXszIjP2Z9gzDmY4Mw5nRGcczYjNOJ4Rn3EyIzHjdEZyxtkMd8b5jNSMixnpGZczMjOuZmRnXM/IzbiZwYzbGfkZdzMKM+5nFGc8zCjNeJxRnvE0ozLjeUZ1xsuM2ozXGfUZbzMaM95nNGd8zGjN+JzRnvE1ozPje0Z3xs/sD+iXuV+8v8z/Ivyy8Iv4y+Iv0i9Lv8i/LP+i/LLyi/rL6i++X9Z+8f+y/kvgl41ftF82f9F/2fol+Mv2L8YvO7+Yv4R+Cf+y+4v1y94vkV/2f7F/OfjF+eXwl+gvR7/Efjn+Jf7LyS+JX05/Sf5y9ov7y/kv/xqgg45AAyAAw2QlK0mSJEmSlSRJkiRJ1kqSlSRJkiRJkiR5JUmSJEmSJEmSJEnSr/kOc5jDHOYwhznsPj/hWS5YKVgtWCtYL9go2CzYKtgu2CmgYLdgr2C/4KDgsOCo4LjgpOC04KzgvOCi4LLgquC64KbgtuCu4L7goeCx4KngueCl4LXgreC94KPgs+Cr4Lv4HySUCD+EUqFM+CmUCxVCpVAlVAs1Qq1QJ9QLDUKj0CQ0Cy3CL6FVaBPahQ6hU+gSuoUeoVfoE/qFAWFQGBKGhd/CH2FEGBXGhHHhrzAhTApTwrQwI8wKc8K8sCAsCkvCsrAirAprwrqwIWwKW8K2sCMg7Ap7wr5wIBwKR8KxcCKcCmfCuXAhXApXwrVwI9wKd8K98CA8Ck/Cs/AivApvwrvwIXwKX8K3/A9SSpQfSqlSpvxUypUKpVKpUqqVGqVWqVPqlQalUWlSmpUW5ZfSqrQp7UqH0ql0Kd1Kj9Kr9Cn9yoAyqAwpw8pv5Y8yoowqY8q48leZUCaVKWVamVFmlTllXllQFpUlZVlZUVaVNWVd2VA2lS1lW9lRUHaVPWVfOVAOlSPlWDlRTpUz5Vy5UC6VK+VauVFulTvlXnlQHpUn5Vl5UV6VN+Vd+VA+lS/lW/8HGSXGD6PUKDN+GuVGhVFpVBnVRo1Ra9QZ9UaD0Wg0Gc1Gi/HLaDXajHajw+g0uoxuo8foNfqMfmPAGDSGjGHjt/HHGDFGjTFj3PhrTBiTxpQxbcwYs8acMW8sGIvGkrFsrBirxpqxbmwYm8aWsW3sGBi7xp6xbxwYh8aRcWycGKfGmXFuXBiXxpVxbdwYt8adcW88GI/Gk/FsvBivxpvxbnwYn8aX8W3/g5wS54dT6pQ5P51yp8KpdKqcaqfGqXXqnHqnwWl0mpxmp8X55bQ6bU670+F0Ol1Ot9Pj9Dp9Tr8z4Aw6Q86w89v544w4o86YM+78dSacSWfKmXZmnFlnzpl3FpxFZ8lZdlacVWfNWXc2nE1ny9l2dhycXWfP2XcOnEPnyDl2TpxT58w5dy6cS+fKuXZunFvnzrl3HpxH58l5dl6cV+fNeXc+nE/ny/n2/0FBSfAjKA3Kgp9BeVARVAZVQXVQE9QGdUF90BA0Bk1Bc9AS/Apag7agPegIOoOuoDvoCXqDvqA/GAgGg6FgOPgd/AlGgtFgLBgP/gYTwWQwFUwHM8FsMBfMBwvBYrAULAcrwWqwFqwHG8FmsBVsBzsBwW6wF+wHB8FhcBQcByfBaXAWnAcXwWVwFVwHN8FtcBfcBw/BY/AUPAcvwWvwFrwHH8Fn8BV8x/+gpCT5kZQmZcnPpDypSCqTqqQ6qUlqk7qkPmlIGpOmpDlpSX4lrUlb0p50JJ1JV9Kd9CS9SV/Snwwkg8lQMpz8Tv4kI8loMpaMJ3+TiWQymUqmk5lkNplL5pOFZDFZSpaTlWQ1WUvWk41kM9lKtpOdhGQ32Uv2k4PkMDlKjpOT5DQ5S86Ti+QyuUquk5vkNrlL7pOH5DF5Sp6Tl+Q1eUvek4/kM/lKvpN/+Me54V09lX4AAAAASUVORK5CYII=";

/// Пробный отчёт в настоящий Telegram — запускается вручную, с ключом:
/// `BRED_REPORT_TOKEN=… BRED_REPORT_CHAT=… cargo test --test report -- --ignored`.
/// Показывает, как отчёт выглядит в чате, не собирая приложение.
#[tokio::test]
#[ignore = "шлёт настоящее сообщение в Telegram"]
async fn real_report_smoke() {
    assert!(
        std::env::var("BRED_REPORT_TOKEN").is_ok() && std::env::var("BRED_REPORT_CHAT").is_ok(),
        "нужны BRED_REPORT_TOKEN и BRED_REPORT_CHAT"
    );
    std::env::remove_var("BRED_REPORT_API");

    let dir = std::env::temp_dir().join(format!("bred-report-real-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Настоящая картинка: PNG 96×64 с градиентом.
    let png = dir.join("пример.png");
    std::fs::write(&png, data_encoding::BASE64.decode(PNG.as_bytes()).unwrap()).unwrap();

    let id = report::Reporter::new()
        .send(
            report::Draft {
                kind: "audio".into(),
                title: "Пробный отчёт: так выглядит обращение из БРЕДа".into(),
                body: "Это проверка формы «Сообщить о проблеме». Здесь будет текст человека: \
                       что делал, что ожидал, что получилось."
                    .into(),
                files: vec![png],
                with_log: true,
            },
            report::Context {
                nick: "проверка".into(),
                account: Id([3u8; 32]),
                device: Id([4u8; 32]),
                network: "соседей 2 · ретранслятор https://euc1-1.relay.n0.iroh.link./".into(),
                log: Some("2026-10-03T14:00:00Z  INFO bred_lib: пример строки журнала".into()),
                sent_at: "03.10.2026, 20:00 GMT+6".into(),
            },
        )
        .await
        .expect("пробный отчёт ушёл");
    println!("отправлен {id}");
    std::fs::remove_dir_all(&dir).ok();
}
