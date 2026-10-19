pub(super) fn is_not_found(error: &serenity::Error) -> bool {
    match error {
        serenity::Error::Http(http_error) => http_error
            .status_code()
            .is_some_and(|status| status.as_u16() == 404),
        _ => false,
    }
}
