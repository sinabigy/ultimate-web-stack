-- Wake idle workers immediately when a job becomes ready (delivered on COMMIT, so a rolled-back
-- enqueue never wakes anyone). Workers still poll as a fallback.
CREATE FUNCTION jobs_notify() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  PERFORM pg_notify('app_jobs', NEW.queue);
  RETURN NULL;
END $$;
CREATE TRIGGER jobs_notify_insert AFTER INSERT ON jobs FOR EACH ROW EXECUTE FUNCTION jobs_notify();
CREATE TRIGGER jobs_notify_requeue AFTER UPDATE OF status ON jobs FOR EACH ROW
    WHEN (NEW.status = 'queued' AND OLD.status IS DISTINCT FROM 'queued') EXECUTE FUNCTION jobs_notify();
