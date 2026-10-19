INSERT INTO settings (key, value)
SELECT 'model_id', json_extract(value, '$.model_id')
FROM settings
WHERE key = 'model_selection';

DELETE FROM settings WHERE key = 'model_selection';
